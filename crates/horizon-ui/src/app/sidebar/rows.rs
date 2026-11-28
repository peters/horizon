//! The work-item rows of the sidebar: a tile in the workspace colour, the name
//! with one line of facts under it, and the one word of the state at the right
//! edge.
//!
//! The row tells the state in words, so a person reads `needs you` and not only a
//! coloured dot. The active row is marked by a tint of the workspace colour and by
//! a bar at the left edge of the sidebar, which a narrow row cannot hide. A row
//! that the pointer is on and that is not active takes the muted fill of the
//! chrome.
//!
//! The panel rows of a work item hang under this row and are folded by default;
//! the chevron of [`super::fold`] opens them.

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, WidgetInfo, pos2, vec2};
use horizon_core::Attention;
use horizon_core::workspace_list::Orchestrator;

use crate::theme;

use super::fold::Mark;
use super::text::{self, SEMIBOLD};
use super::{SidebarWorkspaceInsert, WorkspaceSidebarEntry};

/// The one word under the name of a work-item row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum WorkspaceWord {
    Working,
    NeedsYou,
    Failed,
    Done,
}

impl WorkspaceWord {
    /// The word, in lower case: it stands in small type beside the name.
    pub(in crate::app) const fn text(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::NeedsYou => "needs you",
            Self::Failed => "failed",
            Self::Done => "done",
        }
    }

    /// The colour of the word and of its dot, which is the colour of its state.
    pub(in crate::app) fn color(self) -> Color32 {
        match self {
            Self::Working | Self::Done => theme::ACCENT(),
            Self::NeedsYou => theme::PALETTE_YELLOW(),
            Self::Failed => theme::PALETTE_RED(),
        }
    }
}

/// The height of a row.
pub(super) const ROW_HEIGHT: f32 = 46.0;
/// The horizontal inset of a row's card inside the sidebar.
pub(super) const ROW_INSET: f32 = 8.0;
/// The left edge of the tile inside a row's card, and the side of the tile.
pub(super) const TILE_INSET: f32 = 8.0;
pub(super) const TILE_SIDE: f32 = 30.0;
const CARD_RADIUS: u8 = 10;
/// The gap between the tile and the text of the row.
const TEXT_GAP: f32 = 10.0;
/// The size of the name and of the line under it.
const NAME_SIZE: f32 = 13.0;
const LINE_SIZE: f32 = 12.0;
/// The size of the state word, the side of its dot and the gap before it.
const WORD_SIZE: f32 = 10.5;
const WORD_DOT_SIDE: f32 = 6.0;
const WORD_DOT_GAP: f32 = 5.0;
/// The right inset of the texts of a row, and the gap between the state word and
/// the facts that follow it on the second line.
const WORD_INSET: f32 = 8.0;
const WORD_GAP: f32 = 8.0;
/// The least room that the facts keep after the state word. In less room the
/// second line holds the word alone.
const MIN_FACTS_WIDTH: f32 = 36.0;
/// The least room that the name keeps before the `NEW WINDOW` badge gives way.
const MIN_NAME_WIDTH: f32 = 48.0;
const BADGE_GAP: f32 = 6.0;
/// The bar of the active row: its width and the air above and under it.
const BAR_SIDE: f32 = 3.0;
const BAR_INSET: f32 = 10.0;
/// The radius of the two right corners of the bar.
const BAR_RADIUS: u8 = 3;

/// The fill of a row that the pointer is on, at half strength.
const HOVER_ALPHA: u8 = 128;

/// The card of a row: the row without the sidebar inset.
pub(super) fn card(row: Rect) -> Rect {
    row.shrink2(vec2(ROW_INSET, 0.0))
}

/// The marks of a row that take a press or show a text of their own, which the
/// caller of [`paint`] uses.
pub(super) struct Parts {
    /// The control that opens and folds the panel rows of the row, while it shows.
    pub(super) fold: Option<Rect>,
    /// The box of the name and of the line, which shows the whole text of a row
    /// that cuts it.
    pub(super) text: Rect,
    pub(super) name_elided: bool,
    pub(super) line_elided: bool,
}

/// The tile of a row: the side of [`TILE_SIDE`], centred in the card and 8 in from
/// its left edge.
pub(super) fn tile_rect(card: Rect) -> Rect {
    Rect::from_min_size(
        pos2(card.min.x + TILE_INSET, card.center().y - TILE_SIDE / 2.0),
        vec2(TILE_SIDE, TILE_SIDE),
    )
}

/// Where the texts of a row end: 8 in from the right edge of the card, and 22
/// more while the fold control shows there. The sidebar is narrow, so the state
/// word stands under the name and the name keeps this whole width.
pub(super) fn text_right(card: Rect, mark: Mark) -> f32 {
    card.max.x - WORD_INSET - if mark.shows() { FOLD_SHIFT } else { 0.0 }
}

/// The shift of the texts while the control of the fold shows.
const FOLD_SHIFT: f32 = 22.0;

/// The fill of a row's card: the accent blend while dragged, the tint of the
/// workspace colour for the active row, the muted fill at half strength while the
/// pointer is on another row, or none at rest.
pub(super) fn card_fill(workspace_color: Color32, is_active: bool, hovered: bool, dragging: bool) -> Option<Color32> {
    if dragging {
        Some(theme::blend(theme::PANEL_BG_ALT(), workspace_color, 0.22))
    } else if is_active {
        Some(theme::alpha(workspace_color, 23))
    } else if hovered {
        Some(theme::alpha(theme::PANEL_BG_ALT(), HOVER_ALPHA))
    } else {
        None
    }
}

/// Paints the row of `workspace`: its card, the bar of the active row, the tile,
/// the name, the line of facts, the state word and the fold control. It answers
/// the rectangles of the marks that take a press.
pub(super) fn paint(
    ui: &egui::Ui,
    row: Rect,
    workspace: &WorkspaceSidebarEntry,
    word: Option<WorkspaceWord>,
    mark: Mark,
    hovered: bool,
    dragging: bool,
) -> Parts {
    let card = card(row);
    let painter = ui.painter();
    if let Some(fill) = card_fill(
        workspace.color,
        workspace.is_active,
        hovered && !workspace.is_active,
        dragging,
    ) {
        painter.rect_filled(card, CARD_RADIUS, fill);
    }
    if workspace.is_active && !dragging {
        paint_active_bar(painter, row, workspace.color);
    }

    let tile = tile_rect(card);
    paint_workspace_tile(ui, tile, &workspace.name, workspace.color, workspace.is_active);
    let text_left = tile.max.x + TEXT_GAP;

    let fold = mark.shows().then(|| super::fold::rect(row));
    let text_right = badges_right(ui, card, workspace, text_left, text_right(card, mark));
    let text = paint_text(ui, card, workspace, word, text_left, text_right);

    Parts {
        fold,
        text: text.box_rect,
        name_elided: text.name_elided,
        line_elided: text.line_elided,
    }
}

/// The bar of the active row: 3 wide at the left edge of the sidebar, from 10 under
/// the top of the row to 10 over its bottom.
pub(super) fn active_bar(row: Rect) -> Rect {
    Rect::from_min_max(
        pos2(row.min.x, row.min.y + BAR_INSET),
        pos2(row.min.x + BAR_SIDE, row.max.y - BAR_INSET),
    )
}

/// The bar of the active row at the left edge of the sidebar, with the tint of
/// the workspace colour behind the row.
fn paint_active_bar(painter: &egui::Painter, row: Rect, color: Color32) {
    let bar = active_bar(row);
    let radius = CornerRadius {
        nw: 0,
        ne: BAR_RADIUS,
        sw: 0,
        se: BAR_RADIUS,
    };
    painter.rect_filled(bar, radius, theme::alpha(color, 220));
}

/// The draws the tile of a work item: the initials of its name in the workspace
/// colour. The row of the sidebar and the header of the orchestrator dock show the
/// same tile, so a person reads one mark for one workspace.
pub(in crate::app) fn paint_workspace_tile(ui: &egui::Ui, tile: Rect, name: &str, accent: Color32, is_active: bool) {
    let painter = ui.painter();
    let fill = theme::blend(theme::PANEL_BG_ALT(), accent, if is_active { 0.42 } else { 0.26 });
    painter.rect_filled(tile, 9, fill);
    let letters_color = theme::ensure_terminal_text_contrast(theme::blend(accent, theme::FG(), 0.35), fill);
    let letters = text::line(ui, &text::initials(name), 12.0, text::BOLD, letters_color, tile.width());
    painter.galley(tile.center() - letters.size() / 2.0, letters, letters_color);
}

/// Paints the state word at `left`, centred on `center_y`: the dot in the colour
/// of the state, then the word in the same colour.
fn paint_word(
    ui: &egui::Ui,
    workspace: u64,
    word: WorkspaceWord,
    galley: std::sync::Arc<egui::Galley>,
    left: f32,
    center_y: f32,
) {
    let color = word.color();
    let word_left = left + WORD_DOT_SIDE + WORD_DOT_GAP;
    let right = word_left + galley.size().x;
    let half = galley.size().y / 2.0;
    ui.painter()
        .circle_filled(pos2(left + WORD_DOT_SIDE / 2.0, center_y), WORD_DOT_SIDE / 2.0, color);
    ui.painter().galley(pos2(word_left, center_y - half), galley, color);
    let response = ui.interact(
        Rect::from_min_max(pos2(left, center_y - half), pos2(right, center_y + half)),
        ui.make_persistent_id(("sidebar_work_item_word", workspace)),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Label, true, word.text()));
}

/// What the two texts of a row say about themselves.
struct Text {
    box_rect: Rect,
    name_elided: bool,
    line_elided: bool,
}

/// Paints the name and, under it, the second line: the state word with its dot,
/// and after it the facts while they have room. The name stands alone in the
/// middle of the row when there is no second line.
fn paint_text(
    ui: &egui::Ui,
    card: Rect,
    workspace: &WorkspaceSidebarEntry,
    word: Option<WorkspaceWord>,
    text_left: f32,
    text_right: f32,
) -> Text {
    let facts = workspace.facts.as_str();
    let width = (text_right - text_left).max(1.0);
    let name = text::line(ui, &workspace.name, NAME_SIZE, SEMIBOLD, theme::FG(), width);
    let word_room = (width - WORD_DOT_SIDE - WORD_DOT_GAP).max(1.0);
    let word = word.map(|word| {
        (
            word,
            text::line(ui, word.text(), WORD_SIZE, text::REGULAR, word.color(), word_room),
        )
    });
    let word_width = word.as_ref().map_or(0.0, |(_, galley)| {
        WORD_DOT_SIDE + WORD_DOT_GAP + galley.size().x + WORD_GAP
    });
    let facts_width = width - word_width;
    let line = (!facts.is_empty() && facts_width >= MIN_FACTS_WIDTH)
        .then(|| text::line(ui, facts, LINE_SIZE, text::REGULAR, text::secondary(), facts_width));
    let name_height = name.size().y;
    let second_height = line
        .as_ref()
        .map_or(0.0, |line| line.size().y)
        .max(word.as_ref().map_or(0.0, |(_, galley)| galley.size().y));
    let gap = if second_height > 0.0 { 2.0 } else { 0.0 };
    let top = card.center().y - (name_height + gap + second_height) / 2.0;
    let name_elided = name.elided;
    ui.painter().galley(pos2(text_left, top), name, theme::FG());
    let second_center = top + name_height + gap + second_height / 2.0;
    if let Some((word, galley)) = word {
        paint_word(ui, workspace.id.0, word, galley, text_left, second_center);
    }
    let line_elided = line.as_ref().is_some_and(|line| line.elided);
    if let Some(line) = line {
        let line_top = second_center - line.size().y / 2.0;
        ui.painter()
            .galley(pos2(text_left + word_width, line_top), line, text::secondary());
    }
    Text {
        box_rect: Rect::from_min_size(pos2(text_left, top), vec2(width, name_height + gap + second_height)),
        name_elided,
        line_elided,
    }
}

/// The badge of a detached work item, which stands left of its state word. It
/// answers where the name must end: before the badge when the badge fits, and
/// otherwise where it was.
fn badges_right(ui: &egui::Ui, card: Rect, workspace: &WorkspaceSidebarEntry, text_left: f32, text_right: f32) -> f32 {
    let Some(badge) = workspace
        .detached
        .then(|| text::line(ui, "NEW WINDOW", 8.5, SEMIBOLD, theme::FG_DIM(), 80.0))
    else {
        return text_right;
    };
    let width = badge.size().x;
    if text_right - width - BADGE_GAP - text_left < MIN_NAME_WIDTH {
        return text_right;
    }
    let right = text_right - width;
    ui.painter().galley(
        pos2(right, card.center().y - badge.size().y / 2.0),
        badge,
        theme::FG_DIM(),
    );
    right - BADGE_GAP
}

pub(super) fn paint_workspace_drop_indicator(
    ui: &egui::Ui,
    workspace_rect: Rect,
    insert: SidebarWorkspaceInsert,
    workspace_color: Color32,
) {
    let y = match insert {
        SidebarWorkspaceInsert::Before => workspace_rect.min.y + 1.0,
        SidebarWorkspaceInsert::After => workspace_rect.max.y - 1.0,
    };
    let left = workspace_rect.min.x + 12.0;
    let right = workspace_rect.max.x - 12.0;
    ui.painter().line_segment(
        [Pos2::new(left, y), Pos2::new(right, y)],
        Stroke::new(2.0_f32, theme::alpha(workspace_color, 220)),
    );
}

/// The word under the name of a work-item row, from the state of the row.
pub(super) fn word_of(row: &horizon_core::workspace_list::Row) -> Option<WorkspaceWord> {
    match row.orchestrator {
        Orchestrator::NeedsYou => Some(WorkspaceWord::NeedsYou),
        Orchestrator::Failed => Some(WorkspaceWord::Failed),
        Orchestrator::Working => Some(WorkspaceWord::Working),
        Orchestrator::Done => Some(WorkspaceWord::Done),
        Orchestrator::Quiet => match row.attention {
            Attention::NeedsYou => Some(WorkspaceWord::NeedsYou),
            Attention::Done => Some(WorkspaceWord::Done),
            Attention::Idle | Attention::Working | Attention::Background => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Parts, WorkspaceWord, card, card_fill, paint, word_of};
    use crate::test_egui::DiscardTextures;
    use crate::theme;
    use egui::pos2;

    fn entry(active: bool, detached: bool) -> super::WorkspaceSidebarEntry {
        super::WorkspaceSidebarEntry {
            id: horizon_core::WorkspaceId(3),
            local_id: "local-3".to_owned(),
            name: "Build the shop".to_owned(),
            facts: "1 needs you \u{00B7} on build-1".to_owned(),
            place: horizon_core::WorkspacePlace::Local,
            machine_panels: 0,
            color: theme::ACCENT(),
            is_active: active,
            detached,
            capabilities: crate::app::workspace::WorkspaceLayoutCapabilities {
                can_arrange: true,
                can_detach: false,
                can_remove_empty: false,
            },
            panels: Vec::new(),
            row: horizon_core::workspace_list::Row::local(horizon_core::Attention::Idle),
        }
    }

    #[test]
    fn the_row_keeps_the_geometry_of_the_design() {
        // A row of a sidebar 214 wide: the card is 8 in at each side, the tile is
        // 30 wide and 8 in from the card, and the texts end 8 before the card.
        let row = egui::Rect::from_min_size(pos2(0.0, 100.0), egui::vec2(214.0, super::ROW_HEIGHT));
        let card = card(row);
        assert!((card.min.x - 8.0).abs() < f32::EPSILON);
        assert!((card.max.x - 206.0).abs() < f32::EPSILON);
        assert!((row.height() - 46.0).abs() < f32::EPSILON);
        let tile = super::tile_rect(card);
        assert!((tile.min.x - 16.0).abs() < f32::EPSILON);
        assert!((tile.width() - 30.0).abs() < f32::EPSILON);
        assert!((tile.height() - 30.0).abs() < f32::EPSILON);
        assert!((tile.center().y - card.center().y).abs() < f32::EPSILON);
        assert!((super::text_right(card, super::Mark::Hidden) - 198.0).abs() < f32::EPSILON);
        assert!(
            (super::text_right(card, super::Mark::Open) - 176.0).abs() < f32::EPSILON,
            "the texts end 22 earlier while the control of the fold shows"
        );
        let fold = super::super::fold::rect(row);
        assert!((fold.max.x - (214.0 - 6.0)).abs() < f32::EPSILON);
        assert!((fold.width() - 18.0).abs() < f32::EPSILON);
    }

    /// The row paints its marks inside the card of a sidebar 214 wide.
    fn painted(active: bool, detached: bool, hovered: bool) -> Parts {
        let ctx = egui::Context::default();
        let mut parts = None;
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(214.0, 600.0))),
                    ..egui::RawInput::default()
                },
                |ui| {
                    let row = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(214.0, super::ROW_HEIGHT));
                    let entry = entry(active, detached);
                    parts = Some(paint(
                        ui,
                        row,
                        &entry,
                        Some(WorkspaceWord::NeedsYou),
                        super::Mark::Open,
                        hovered,
                        false,
                    ));
                },
            )
            .discard_textures();
        parts.expect("the parts of the row")
    }

    #[test]
    fn the_word_stands_under_the_name_and_the_name_keeps_the_width_of_the_row() {
        let parts = painted(true, false, false);
        let card = super::card(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(214.0, super::ROW_HEIGHT),
        ));
        // The texts start 10 after the tile and end where the fold control begins,
        // so no word at the right edge takes room from the name.
        let left = super::tile_rect(card).max.x + super::TEXT_GAP;
        assert!((parts.text.min.x - left).abs() < f32::EPSILON);
        assert!(
            (parts.text.max.x - super::text_right(card, super::Mark::Open)).abs() < f32::EPSILON,
            "the name keeps the whole width of the row: {:?}",
            parts.text
        );
        // A row with a word has two lines, also when it has no fact.
        assert!(
            parts.text.height() > 22.0,
            "the word is the second line: {:?}",
            parts.text
        );
        assert!(!parts.name_elided, "a short name is whole beside no word");
        assert_eq!(
            parts.fold.map(|fold| fold.min.x),
            Some(214.0 - 24.0),
            "the fold control stands at the right end of the row"
        );
    }

    #[test]
    fn the_active_row_is_marked_with_a_tint_and_the_rows_under_the_pointer_with_a_muted_fill() {
        let accent = egui::Color32::from_rgb(137, 180, 250);
        assert_eq!(
            card_fill(accent, true, false, false),
            Some(theme::alpha(accent, 23)),
            "the active row takes a tint of its workspace colour"
        );
        assert_eq!(
            card_fill(accent, false, true, false),
            Some(theme::alpha(theme::PANEL_BG_ALT(), 128)),
            "another row under the pointer takes the muted fill at half strength"
        );
        assert_eq!(card_fill(accent, false, false, false), None);
        assert_eq!(
            card_fill(accent, false, false, true),
            Some(theme::blend(theme::PANEL_BG_ALT(), accent, 0.22)),
            "a dragged row keeps the blend of today"
        );
    }

    #[test]
    fn the_word_comes_from_the_orchestrator_then_from_the_attention_state() {
        use horizon_core::workspace_list::{Orchestrator, Row};
        let row = |orchestrator, attention| Row {
            attention,
            ..Row::placed(
                attention,
                horizon_core::workspace_list::Waiting::default(),
                None,
                orchestrator,
            )
        };
        assert_eq!(
            word_of(&row(Orchestrator::Working, horizon_core::Attention::Idle)),
            Some(WorkspaceWord::Working)
        );
        assert_eq!(
            word_of(&row(Orchestrator::Failed, horizon_core::Attention::Idle)),
            Some(WorkspaceWord::Failed)
        );
        assert_eq!(
            word_of(&row(Orchestrator::Done, horizon_core::Attention::Idle)),
            Some(WorkspaceWord::Done)
        );
        assert_eq!(
            word_of(&row(Orchestrator::Quiet, horizon_core::Attention::NeedsYou)),
            Some(WorkspaceWord::NeedsYou),
            "a panel that waits marks the row even with no word of its orchestrator"
        );
        assert_eq!(
            word_of(&row(Orchestrator::Quiet, horizon_core::Attention::Done)),
            Some(WorkspaceWord::Done)
        );
        assert_eq!(
            word_of(&row(Orchestrator::Quiet, horizon_core::Attention::Working)),
            None
        );
        assert_eq!(word_of(&row(Orchestrator::Quiet, horizon_core::Attention::Idle)), None);
    }
}

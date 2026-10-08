//! The production cloud's header: one status sentence, spend, connections, the main
//! action and the drawer toggle, with the stage track along the bottom edge.
use super::super::Stage;
use super::status::{Primary, Status, Tone, Track};
use crate::theme;
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};

/// Height of the stage track along the header's bottom edge.
const TRACK_HEIGHT: f32 = 5.0;
/// Vertical centre of the title-row controls, level with the close button.
const CONTROLS_Y: f32 = 35.0;
/// Room the title keeps before optional controls are dropped.
const TITLE_MIN: f32 = 170.0;
const TITLE_LEFT: f32 = 64.0;

/// Which way the local network bridge is going, for its indicator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::cloud_panel) enum Sharing {
    Off,
    Paused,
    /// This computer moved to another network; nothing is shared until the owner shares it.
    Moved,
    Starting,
    Open(usize),
    /// The bridge lost its connection and is trying again.
    Reconnecting,
    Failed,
}

pub(in crate::app::cloud_panel) struct Indicators {
    pub running: usize,
    pub terminals: usize,
    /// `None` when the profile has no desktop.
    pub desktop: Option<bool>,
    pub sharing: Sharing,
    pub companions: usize,
}

pub(in crate::app::cloud_panel) struct Spend {
    /// "$0.320/h · $1.02 run · $8.86 total", or what is known instead.
    pub line: String,
    pub explanation: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::cloud_panel) enum StripAction {
    Primary(Primary),
    ToggleDrawer,
    Layout(Option<horizon_core::WorkspaceLayout>),
}

/// The workspace's panel arrangement, offered on the header of a cloud that holds or takes panels.
pub(in crate::app::cloud_panel) struct LayoutControls {
    pub selected: Option<horizon_core::WorkspaceLayout>,
    pub color: Color32,
}

/// Four buttons and the gaps between them, as the workspace toolbar draws them.
const LAYOUT_WIDTH: f32 = 208.0;
const LAYOUT_HEIGHT: f32 = 24.0;
/// Below this header width the status sentence keeps the row; Manage still has the buttons.
const LAYOUT_MIN_HEADER: f32 = 700.0;

pub(in crate::app::cloud_panel) struct Strip {
    /// Width from the header's right edge that the title must leave free.
    pub reserved: f32,
    pub action: Option<StripAction>,
    /// For example `RunPod / cpu  ·  EU-RO-1  ·  8 vCPU · 32 GB`, set by the caller.
    pub subtitle: String,
}

/// Where a repeating animation is, in [0, 1), at `rate` cycles per second.
#[expect(clippy::cast_possible_truncation, reason = "a phase in [0, 1) fits f32")]
pub(super) fn cycle(time: f64, rate: f64) -> f32 {
    (time * rate).fract() as f32
}

pub(super) fn tone_color(tone: Tone) -> Color32 {
    match tone {
        Tone::Idle => theme::FG_DIM(),
        Tone::Live => theme::PALETTE_CYAN(),
        Tone::Ready => theme::PALETTE_GREEN(),
        Tone::Attention => theme::PALETTE_YELLOW(),
        Tone::Failed => theme::PALETTE_RED(),
    }
}

/// One hue per part of the path (this computer, provider, worker), as the ready timeline.
pub(super) fn stage_color(stage: Stage) -> Color32 {
    let (hue, shade) = match stage {
        Stage::Validate => (theme::ACCENT(), 0.0),
        Stage::Build => (theme::ACCENT(), 0.3),
        Stage::Push | Stage::Replace => (theme::ACCENT(), 0.5),
        Stage::Provision => (theme::PALETTE_YELLOW(), 0.45),
        Stage::Readiness => (theme::PALETTE_CYAN(), 0.0),
        Stage::Worktrees => (theme::PALETTE_GREEN(), 0.45),
        Stage::Sessions => (theme::FG_DIM(), 0.0),
        Stage::Ready => (theme::PALETTE_GREEN(), 0.0),
        Stage::ReleaseDevices | Stage::DeleteWorker | Stage::DeleteStorage | Stage::Deleted => {
            (theme::PALETTE_RED(), 0.35)
        }
        Stage::Stopped | Stage::Stopping => (theme::PALETTE_YELLOW(), 0.3),
    };
    theme::blend(hue, theme::PANEL_BG(), shade)
}

pub(super) fn show(
    ui: &mut egui::Ui,
    header: Rect,
    status: &Status,
    indicators: &Indicators,
    spend: &Spend,
    drawer_open: bool,
    layout: Option<LayoutControls>,
) -> Strip {
    let mut action = None;
    let y = header.top() + CONTROLS_Y;
    let close_left = header.right() - 42.0;
    // Expand sits next to the close button in every width.
    let expand = Rect::from_center_size(pos2(close_left - 20.0, y), Vec2::splat(30.0));
    if expand_button(ui, expand, drawer_open) {
        action = Some(StripAction::ToggleDrawer);
    }
    let mut left = expand.left() - 12.0;
    let room = |left: f32, width: f32| left - width - (header.left() + TITLE_LEFT) >= TITLE_MIN;
    if let Some(primary) = status.primary {
        let width = button_width(ui, primary.label());
        if room(left, width) {
            let rect = Rect::from_min_max(pos2(left - width, y - 15.0), pos2(left, y + 15.0));
            if primary_button(ui, rect, primary) {
                action = Some(StripAction::Primary(primary));
            }
            left -= width + 22.0;
        }
    }
    let placed = place(ui, indicators);
    let width = placed.iter().map(|placed| placed.width).sum::<f32>();
    // Spend is dropped first: it shows only once the indicators have their room.
    let indicators_fit = room(left, width);
    if indicators_fit {
        paint_indicators(ui, pos2(left - width, y), placed);
        left -= width + 22.0;
    }
    let spend_galley = ui
        .painter()
        .layout_no_wrap(spend.line.clone(), FontId::proportional(14.0), theme::FG_SOFT());
    if indicators_fit && room(left, spend_galley.size().x) {
        let rect = Rect::from_min_size(
            pos2(left - spend_galley.size().x, y - spend_galley.size().y / 2.0),
            spend_galley.size(),
        );
        ui.painter().galley(rect.min, spend_galley, theme::FG_SOFT());
        let spoken = format!("{}. {}", spend.line, spend.explanation);
        ui.interact(rect, ui.id().with("spend"), Sense::hover())
            .on_hover_text(&spend.explanation)
            .widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Label, true, spoken.trim_end_matches(['.', ' ']))
            });
        left = rect.left() - 24.0;
    }
    let mut reserved_row = 0.0;
    if let Some(mut controls) = layout.filter(|_| header.width() >= LAYOUT_MIN_HEADER) {
        let row_y = header.bottom() - TRACK_HEIGHT - 15.0;
        let rect = Rect::from_min_size(
            pos2(header.right() - 26.0 - LAYOUT_WIDTH, row_y - LAYOUT_HEIGHT / 2.0),
            vec2(LAYOUT_WIDTH, LAYOUT_HEIGHT),
        );
        let changed = ui
            .scope_builder(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| crate::app::workspace::workspace_layout_buttons(ui, &mut controls.selected, controls.color),
            )
            .inner;
        if changed {
            action = Some(StripAction::Layout(controls.selected));
        }
        reserved_row = LAYOUT_WIDTH + 16.0;
    }
    status_line(ui, header, status, reserved_row);
    let track = Rect::from_min_max(
        pos2(header.left() + 1.0, header.bottom() - TRACK_HEIGHT),
        pos2(header.right() - 1.0, header.bottom()),
    );
    paint_track(ui, track, &status.track);
    Strip {
        reserved: header.right() - left,
        action,
        subtitle: String::new(),
    }
}

fn status_line(ui: &egui::Ui, header: Rect, status: &Status, reserved: f32) {
    let painter = ui.painter();
    let y = header.bottom() - TRACK_HEIGHT - 15.0;
    let color = tone_color(status.tone);
    let dot = pos2(header.left() + 26.0, y);
    if status.live() {
        // Quiet while it runs: the elapsed time and ETA tick once a second.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    }
    painter.circle_filled(dot, 4.0, color);
    let failed = status.tone == Tone::Failed;
    let meaning = status
        .failure
        .as_ref()
        .and_then(|failure| failure.meaning)
        .map(|meaning| format!("—  {meaning}"));
    let parts = [
        (status.verb.as_str(), 15.5, color),
        (
            status.numbers.as_str(),
            14.5,
            if failed { theme::FG() } else { theme::FG_SOFT() },
        ),
        (status.tail.as_str(), 13.5, theme::FG_DIM()),
        (meaning.as_deref().unwrap_or_default(), 13.0, theme::FG_SOFT()),
    ];
    let start = header.left() + 40.0;
    let end = header.right() - 26.0 - reserved;
    // Each part is laid out once: its width decides the fit, and it is painted as is
    // unless it must be cut.
    let laid: Vec<_> = parts
        .iter()
        .enumerate()
        .filter(|(_, (text, ..))| !text.is_empty())
        .map(|(index, (text, size, color))| {
            let galley = painter.layout_no_wrap((*text).to_owned(), FontId::proportional(*size), *color);
            (index, *text, *size, *color, galley)
        })
        .collect();
    // The sentence comes first: the right-hand summary shows only beside its verb and numbers.
    let lead = laid
        .iter()
        .filter(|(index, ..)| *index < 2)
        .map(|(.., galley)| galley.size().x + 12.0)
        .sum::<f32>();
    let right = (!status.right.is_empty())
        .then(|| painter.layout_no_wrap(status.right.clone(), FontId::proportional(13.0), theme::FG_DIM()));
    let limit = match right {
        Some(right) if start + lead + 20.0 + right.size().x <= end => {
            let width = right.size().x;
            painter.galley(pos2(end - width, y - right.size().y / 2.0), right, theme::FG_DIM());
            end - width - 20.0
        }
        _ => end,
    };
    let mut x = start;
    for (_, text, size, color, galley) in laid {
        let room = limit - x;
        if room < 28.0 {
            break;
        }
        let galley = if galley.size().x <= room {
            galley
        } else {
            // Too long for the room left: end with an ellipsis rather than a cut.
            let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), FontId::proportional(size), color);
            job.wrap = egui::text::TextWrapping::truncate_at_width(room);
            painter.layout_job(job)
        };
        let height = galley.size().y;
        let width = galley.size().x;
        painter.galley(pos2(x, y - height / 2.0), galley, color);
        x += width + 12.0;
    }
    // Painted, so it is registered once as the whole sentence, cut or not.
    ui.interact(
        Rect::from_min_max(pos2(start, y - 9.0), pos2(end, y + 9.0)),
        ui.id().with("status-line"),
        Sense::hover(),
    )
    .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, spoken(status, meaning.as_deref())));
}

/// The status as one sentence for a screen reader.
fn spoken(status: &Status, meaning: Option<&str>) -> String {
    [
        status.verb.as_str(),
        status.numbers.as_str(),
        status.tail.as_str(),
        meaning.map_or("", |meaning| meaning.trim_start_matches(['—', ' '])),
        status.right.as_str(),
    ]
    .into_iter()
    .map(|part| part.trim().trim_end_matches('.'))
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(". ")
}

/// Segments with the finished stages in their colours, the running one filled to its
/// measured share with a travelling sheen, and a failed one red.
pub(super) fn paint_track(ui: &egui::Ui, rect: Rect, track: &Track) {
    let painter = ui.painter();
    let count = track.stages.len().max(1);
    let gap = 2.0;
    let slots = crate::app::util::usize_to_f32(count);
    let width = (rect.width() - gap * (slots - 1.0)) / slots;
    let radius = CornerRadius::same(if rect.height() >= 8.0 { 4 } else { 2 });
    let mut segments = Vec::with_capacity(count);
    for (index, stage) in track.stages.iter().enumerate() {
        let segment = Rect::from_min_size(
            pos2(
                rect.left() + crate::app::util::usize_to_f32(index) * (width + gap),
                rect.top(),
            ),
            vec2(width, rect.height()),
        );
        painter.rect_filled(segment, radius, theme::BORDER_SUBTLE());
        let color = stage_color(*stage);
        if track.skipped.contains(stage) && track.current != Some(index) {
            // A step that never runs: a thin rule through the empty segment, not a colour.
            let rule = Rect::from_center_size(segment.center(), vec2(width, 1.0_f32.min(rect.height())));
            painter.rect_filled(rule, 0, theme::FG_DIM());
        } else if index < track.finished && track.current != Some(index) {
            let color = if track.faded {
                theme::blend(color, theme::PANEL_BG(), 0.45)
            } else {
                color
            };
            painter.rect_filled(segment, radius, color);
        } else if track.current == Some(index) {
            if track.failed {
                painter.rect_filled(segment, radius, theme::PALETTE_RED());
            } else if let Some(fraction) = track.fraction {
                let filled = Rect::from_min_size(segment.min, vec2(width * fraction.clamp(0.04, 1.0), rect.height()));
                painter.rect_filled(filled, radius, color);
            } else {
                // Unmeasured: a half-tone segment, still, rather than a pulse.
                painter.rect_filled(segment, radius, theme::blend(theme::BORDER_SUBTLE(), color, 0.35));
            }
        }
        segments.push(segment);
    }
    let hover = ui.interact(
        rect.expand2(vec2(0.0, 3.0)),
        ui.id().with("stage-track"),
        Sense::hover(),
    );
    if let Some(pointer) = hover.hover_pos()
        && let Some(index) = segments
            .iter()
            .position(|segment| segment.x_range().contains(pointer.x))
    {
        let stage = track.stages[index];
        if track.skipped.contains(&stage) {
            hover.on_hover_text_at_pointer(format!("{} · skipped", stage.label()));
        } else {
            hover.on_hover_text_at_pointer(stage.label());
        }
    }
}

fn expand_button(ui: &egui::Ui, rect: Rect, open: bool) -> bool {
    let response = ui.interact(rect, ui.id().with("drawer-toggle"), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            if open {
                "Hide cloud details"
            } else {
                "Show cloud details"
            },
        )
    });
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        8,
        if response.hovered() {
            theme::alpha(theme::ACCENT(), 50)
        } else {
            theme::alpha(theme::FG_DIM(), 25)
        },
    );
    chevron(painter, rect.center(), open, theme::FG());
    response
        .on_hover_text(if open {
            "Hide details"
        } else {
            "Details: machine, cost, connections, output and management"
        })
        .clicked()
}

/// Points down to open and up to close.
pub(super) fn chevron(painter: &egui::Painter, center: Pos2, open: bool, color: Color32) {
    let size = 5.0;
    let direction = if open { -1.0 } else { 1.0 };
    painter.add(Shape::line(
        vec![
            center + vec2(-size, -size * 0.5 * direction),
            center + vec2(0.0, size * 0.5 * direction),
            center + vec2(size, -size * 0.5 * direction),
        ],
        Stroke::new(2.0, color),
    ));
}

fn button_width(ui: &egui::Ui, label: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(13.5), theme::FG())
        .size()
        .x
        + 26.0
}

fn primary_button(ui: &egui::Ui, rect: Rect, primary: Primary) -> bool {
    // Keyed by the action: a press begun on Cancel cannot complete as the Retry deploy
    // that replaces it when the operation fails mid-click.
    let response = ui.interact(rect, ui.id().with(("primary", primary)), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, primary.label()));
    let color = if primary.destructive() {
        theme::PALETTE_RED()
    } else if primary.emphasized() {
        theme::ACCENT()
    } else {
        theme::FG_SOFT()
    };
    let painter = ui.painter();
    let hovered = response.hovered();
    if primary.emphasized() {
        painter.rect_filled(
            rect,
            7,
            theme::blend(theme::PANEL_BG(), color, if hovered { 0.38 } else { 0.28 }),
        );
    } else if hovered {
        painter.rect_filled(rect, 7, theme::alpha(color, 30));
    }
    painter.rect_stroke(
        rect,
        7,
        Stroke::new(1.0, theme::alpha(color, if primary.emphasized() { 160 } else { 110 })),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        primary.label(),
        FontId::proportional(13.5),
        if primary.emphasized() { theme::FG() } else { color },
    );
    response.clicked()
}

struct Item {
    glyph: fn(&egui::Painter, Pos2, Color32),
    label: String,
    active: bool,
    color: Option<Color32>,
    tip: String,
}

/// Indicators show only while there is something to report, so an idle or undeployed
/// cloud does not read as a row of disabled buttons.
fn items(indicators: &Indicators) -> Vec<Item> {
    let mut items = Vec::new();
    if indicators.running > 0 {
        items.push(Item {
            glyph: glyph_terminal,
            label: format!("{}/{}", indicators.running, indicators.terminals),
            active: true,
            color: None,
            tip: format!("{} of {} terminals running", indicators.running, indicators.terminals),
        });
    }
    if indicators.desktop == Some(true) {
        items.push(Item {
            glyph: glyph_desktop,
            label: "Desktop".into(),
            active: true,
            color: None,
            tip: "Desktop tunnel connected".into(),
        });
    }
    items.extend(network_item(indicators.sharing));
    items.extend(companions_item(indicators.companions));
    items
}

/// The local network shows only while it is shared or needs attention.
fn network_item(sharing: Sharing) -> Option<Item> {
    let (label, active, color, tip) = match sharing {
        Sharing::Off => return None,
        Sharing::Paused => (
            "Network".to_owned(),
            false,
            Some(theme::PALETTE_YELLOW()),
            "Local network sharing paused while disconnected".to_owned(),
        ),
        Sharing::Moved => (
            "Network".to_owned(),
            false,
            Some(theme::PALETTE_YELLOW()),
            "Local network sharing stopped: this computer moved to another network. Share it again in Connections"
                .to_owned(),
        ),
        Sharing::Starting => (
            "Network".to_owned(),
            true,
            None,
            "Connecting the local network bridge".to_owned(),
        ),
        Sharing::Reconnecting => (
            "Network".to_owned(),
            true,
            Some(theme::PALETTE_YELLOW()),
            "Local network bridge reconnecting; see Connections".to_owned(),
        ),
        Sharing::Open(count) => (
            format!("Network {count}"),
            true,
            Some(theme::PALETTE_GREEN()),
            format!("Sharing the local network · {count} open"),
        ),
        Sharing::Failed => (
            "Network".to_owned(),
            false,
            Some(theme::PALETTE_RED()),
            "Local network sharing failed; see Connections".to_owned(),
        ),
    };
    Some(Item {
        glyph: glyph_network,
        label,
        active,
        color,
        tip,
    })
}

/// Companion clouds appear only once one is selected.
fn companions_item(companions: usize) -> Option<Item> {
    (companions > 0).then(|| Item {
        glyph: glyph_link,
        label: format!("Companions {companions}"),
        active: true,
        color: None,
        tip: match companions {
            1 => "1 companion cloud selected".into(),
            count => format!("{count} companion clouds selected"),
        },
    })
}

/// An indicator laid out once per frame: measured and painted from the same galley.
struct Placed {
    item: Item,
    color: Color32,
    label: Option<std::sync::Arc<egui::Galley>>,
    width: f32,
}

fn place(ui: &egui::Ui, indicators: &Indicators) -> Vec<Placed> {
    items(indicators)
        .into_iter()
        .map(|item| {
            let color = item.color.unwrap_or(if item.active {
                theme::FG_SOFT()
            } else {
                theme::alpha(theme::FG_DIM(), 150)
            });
            let label = (!item.label.is_empty()).then(|| {
                ui.painter()
                    .layout_no_wrap(item.label.clone(), FontId::proportional(12.5), color)
            });
            let width = 20.0 + label.as_ref().map_or(8.0, |galley| galley.size().x + 12.0);
            Placed {
                item,
                color,
                label,
                width,
            }
        })
        .collect()
}

fn paint_indicators(ui: &egui::Ui, left_center: Pos2, placed: Vec<Placed>) {
    let mut x = left_center.x;
    for (index, placed) in placed.into_iter().enumerate() {
        (placed.item.glyph)(ui.painter(), pos2(x + 8.0, left_center.y), placed.color);
        if let Some(galley) = placed.label {
            let top = left_center.y - galley.size().y / 2.0;
            ui.painter().galley(pos2(x + 20.0, top), galley, placed.color);
        }
        let rect = Rect::from_min_size(pos2(x, left_center.y - 11.0), vec2(placed.width, 22.0));
        let tip = placed.item.tip;
        ui.interact(rect, ui.id().with(("indicator", index)), Sense::hover())
            .on_hover_text(&tip)
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &tip));
        x += placed.width;
    }
}

fn glyph_terminal(painter: &egui::Painter, center: Pos2, color: Color32) {
    let rect = Rect::from_center_size(center, vec2(16.0, 12.0));
    let stroke = Stroke::new(1.4, color);
    painter.rect_stroke(rect, 2, stroke, StrokeKind::Inside);
    painter.line_segment([rect.min + vec2(3.0, 3.5), rect.min + vec2(6.0, 6.0)], stroke);
    painter.line_segment([rect.min + vec2(6.0, 6.0), rect.min + vec2(3.0, 8.5)], stroke);
}

fn glyph_desktop(painter: &egui::Painter, center: Pos2, color: Color32) {
    let stroke = Stroke::new(1.4, color);
    painter.rect_stroke(
        Rect::from_center_size(center + vec2(0.0, -1.5), vec2(16.0, 10.0)),
        1,
        stroke,
        StrokeKind::Inside,
    );
    painter.line_segment([center + vec2(-4.0, 6.0), center + vec2(4.0, 6.0)], stroke);
}

fn glyph_network(painter: &egui::Painter, center: Pos2, color: Color32) {
    let stroke = Stroke::new(1.4, color);
    for offset in [vec2(-6.0, 4.0), vec2(6.0, 4.0), vec2(0.0, -5.0)] {
        painter.circle_stroke(center + offset, 2.3, stroke);
    }
    painter.line_segment(
        [center + vec2(0.0, -2.7), center + vec2(0.0, 1.0)],
        Stroke::new(1.2, color),
    );
    painter.line_segment(
        [center + vec2(-6.0, 1.7), center + vec2(6.0, 1.7)],
        Stroke::new(1.2, color),
    );
}

fn glyph_link(painter: &egui::Painter, center: Pos2, color: Color32) {
    let stroke = Stroke::new(1.4, color);
    painter.circle_stroke(center + vec2(-3.5, 0.0), 4.0, stroke);
    painter.circle_stroke(center + vec2(3.5, 0.0), 4.0, stroke);
}

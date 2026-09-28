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
    Starting,
    Open(usize),
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
}

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

/// A smooth 0..1 breathing value at `rate` radians per second.
#[expect(clippy::cast_possible_truncation, reason = "a value in [0, 1] fits f32")]
pub(super) fn breathe(time: f64, rate: f64) -> f32 {
    ((time * rate).sin() * 0.5 + 0.5).clamp(0.0, 1.0) as f32
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
        if room(left, width)
            && primary_button(
                ui,
                Rect::from_min_max(pos2(left - width, y - 15.0), pos2(left, y + 15.0)),
                primary,
            )
        {
            action = Some(StripAction::Primary(primary));
        }
        left -= width + 22.0;
    }
    let width = indicators_width(ui, indicators);
    if room(left, width) {
        paint_indicators(ui, pos2(left - width, y), indicators);
        left -= width + 22.0;
    }
    let spend_galley = ui
        .painter()
        .layout_no_wrap(spend.line.clone(), FontId::proportional(14.0), theme::FG_SOFT());
    if room(left, spend_galley.size().x) {
        let rect = Rect::from_min_size(
            pos2(left - spend_galley.size().x, y - spend_galley.size().y / 2.0),
            spend_galley.size(),
        );
        ui.painter().galley(rect.min, spend_galley, theme::FG_SOFT());
        ui.interact(rect, ui.id().with("spend"), Sense::hover())
            .on_hover_text(&spend.explanation);
        left = rect.left() - 24.0;
    }
    status_line(ui, header, status);
    let track = Rect::from_min_max(
        pos2(header.left() + 1.0, header.bottom() - TRACK_HEIGHT),
        pos2(header.right() - 1.0, header.bottom()),
    );
    paint_track(ui, track, &status.track, status.live());
    Strip {
        reserved: header.right() - left,
        action,
        subtitle: String::new(),
    }
}

fn status_line(ui: &egui::Ui, header: Rect, status: &Status) {
    let painter = ui.painter();
    let y = header.bottom() - TRACK_HEIGHT - 15.0;
    let color = tone_color(status.tone);
    let dot = pos2(header.left() + 26.0, y);
    if status.live() {
        // The dot breathes while an operation runs.
        let pulse = breathe(ui.input(|input| input.time), 2.2);
        painter.circle_filled(
            dot,
            4.0 + 5.0 * pulse,
            theme::alpha(color, 60).gamma_multiply(1.0 - pulse),
        );
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
    painter.circle_filled(dot, 4.0, color);
    let right_left = if status.right.is_empty() {
        header.right() - 26.0
    } else {
        let right = painter.layout_no_wrap(status.right.clone(), FontId::proportional(13.0), theme::FG_DIM());
        let left = header.right() - 26.0 - right.size().x;
        painter.galley(pos2(left, y - right.size().y / 2.0), right, theme::FG_DIM());
        left
    };
    let clip = painter.with_clip_rect(Rect::from_min_max(
        pos2(header.left(), header.bottom() - TRACK_HEIGHT - 32.0),
        pos2(right_left - 20.0, header.bottom() - TRACK_HEIGHT),
    ));
    let mut x = header.left() + 40.0;
    let failed = status.tone == Tone::Failed;
    for (text, size, color, mono) in [
        (status.verb.as_str(), 15.5, color, false),
        (
            status.numbers.as_str(),
            14.5,
            if failed { theme::FG() } else { theme::FG_SOFT() },
            false,
        ),
        (status.tail.as_str(), 13.5, theme::FG_DIM(), false),
    ] {
        if text.is_empty() {
            continue;
        }
        let font = if mono {
            FontId::monospace(size)
        } else {
            FontId::proportional(size)
        };
        let rect = clip.text(pos2(x, y), Align2::LEFT_CENTER, text, font, color);
        x = rect.right() + 12.0;
    }
    if let Some(meaning) = status.failure.as_ref().and_then(|failure| failure.meaning) {
        clip.text(
            pos2(x + 2.0, y),
            Align2::LEFT_CENTER,
            format!("—  {meaning}"),
            FontId::proportional(13.0),
            theme::FG_SOFT(),
        );
    }
}

/// Segments with the finished stages in their colours, the running one filled to its
/// measured share with a travelling sheen, and a failed one red.
pub(super) fn paint_track(ui: &egui::Ui, rect: Rect, track: &Track, live: bool) {
    let painter = ui.painter();
    let count = track.stages.len().max(1);
    let gap = 2.0;
    let slots = crate::app::util::usize_to_f32(count);
    let width = (rect.width() - gap * (slots - 1.0)) / slots;
    let radius = CornerRadius::same(if rect.height() >= 8.0 { 4 } else { 2 });
    let time = ui.input(|input| input.time);
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
        if index < track.finished && track.current != Some(index) {
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
                let pulse = breathe(time, 2.5);
                painter.rect_filled(
                    segment,
                    radius,
                    theme::blend(theme::BORDER_SUBTLE(), color, 0.35 + 0.4 * pulse),
                );
            }
            if live && !track.failed {
                let x = segment.left() + width * cycle(time, 0.6);
                let sheen = Rect::from_min_max(
                    pos2((x - 14.0).max(segment.left()), segment.top()),
                    pos2((x + 14.0).min(segment.right()), segment.bottom()),
                );
                painter.rect_filled(sheen, radius, theme::alpha(Color32::WHITE, 28));
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
        hover.on_hover_text_at_pointer(track.stages[index].label());
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
    let response = ui.interact(rect, ui.id().with("primary"), Sense::click());
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

fn items(indicators: &Indicators) -> Vec<Item> {
    let mut items = vec![Item {
        glyph: glyph_terminal,
        label: if indicators.terminals == 0 {
            String::new()
        } else {
            format!("{}/{}", indicators.running, indicators.terminals)
        },
        active: indicators.running > 0,
        color: None,
        tip: match indicators.terminals {
            0 => "No terminals attached".into(),
            total => format!("{} of {total} terminals running", indicators.running),
        },
    }];
    if let Some(connected) = indicators.desktop {
        items.push(Item {
            glyph: glyph_desktop,
            label: String::new(),
            active: connected,
            color: None,
            tip: if connected {
                "Desktop tunnel connected".into()
            } else {
                "Desktop tunnel not connected".into()
            },
        });
    }
    let (label, active, color, tip) = match indicators.sharing {
        Sharing::Off => (String::new(), false, None, "Local network not shared".to_owned()),
        Sharing::Paused => (
            String::new(),
            false,
            Some(theme::PALETTE_YELLOW()),
            "Local network sharing paused while disconnected".to_owned(),
        ),
        Sharing::Starting => (
            String::new(),
            true,
            None,
            "Connecting the local network bridge".to_owned(),
        ),
        Sharing::Open(count) => (
            count.to_string(),
            true,
            Some(theme::PALETTE_GREEN()),
            format!("Sharing the local network · {count} open"),
        ),
        Sharing::Failed => (
            String::new(),
            false,
            Some(theme::PALETTE_RED()),
            "Local network sharing failed; see Connections".to_owned(),
        ),
    };
    items.push(Item {
        glyph: glyph_network,
        label,
        active,
        color,
        tip,
    });
    items.push(Item {
        glyph: glyph_link,
        label: if indicators.companions == 0 {
            String::new()
        } else {
            indicators.companions.to_string()
        },
        active: indicators.companions > 0,
        color: None,
        tip: match indicators.companions {
            0 => "No companion clouds selected".into(),
            1 => "1 companion cloud selected".into(),
            count => format!("{count} companion clouds selected"),
        },
    });
    items
}

fn item_width(ui: &egui::Ui, item: &Item) -> f32 {
    20.0 + if item.label.is_empty() {
        8.0
    } else {
        ui.painter()
            .layout_no_wrap(item.label.clone(), FontId::proportional(12.5), theme::FG())
            .size()
            .x
            + 12.0
    }
}

fn indicators_width(ui: &egui::Ui, indicators: &Indicators) -> f32 {
    items(indicators).iter().map(|item| item_width(ui, item)).sum()
}

fn paint_indicators(ui: &egui::Ui, left_center: Pos2, indicators: &Indicators) {
    let mut x = left_center.x;
    for item in items(indicators) {
        let width = item_width(ui, &item);
        let color = item.color.unwrap_or(if item.active {
            theme::FG_SOFT()
        } else {
            theme::alpha(theme::FG_DIM(), 150)
        });
        (item.glyph)(ui.painter(), pos2(x + 8.0, left_center.y), color);
        if !item.label.is_empty() {
            ui.painter().text(
                pos2(x + 20.0, left_center.y),
                Align2::LEFT_CENTER,
                &item.label,
                FontId::proportional(12.5),
                color,
            );
        }
        let rect = Rect::from_min_size(pos2(x, left_center.y - 11.0), vec2(width, 22.0));
        ui.interact(rect, ui.id().with(("indicator", x.to_bits())), Sense::hover())
            .on_hover_text(item.tip);
        x += width;
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

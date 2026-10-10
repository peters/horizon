//! A cloud operation's output as a first-class view: grouped by step, timed, with
//! failures highlighted. The cause is read beside the steps, not again under the log.
use super::super::{LineKind, LogLine, Runtime, Stage};
use super::strip::stage_color;
use crate::theme;
use egui::{Align, FontId, Layout, Rect, RichText, Stroke, UiBuilder, Vec2};
use std::collections::HashMap;

const TEXT_SIZE: f32 = 13.0;
/// The least the log shows: a few one-row lines, however little room it was given.
const MIN_LOG_HEIGHT: f32 = 4.0 * (TEXT_SIZE + 3.0 + ROW_GAP);

/// The log in `height`. `place` keeps the scroll position of the body's log apart
/// from the drawer's.
pub(super) fn show(ui: &mut egui::Ui, id: u32, place: &str, runtime: &mut Runtime, height: f32) {
    let frame = egui::Frame::new()
        .fill(theme::BG())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 10));
    let inner = (height - frame.total_margin().sum().y).max(MIN_LOG_HEIGHT);
    // Another view scrolled up holds new lines aside; this one, still at the end, shows them.
    let follows = runtime.unpinned_last & view_bit(place) == 0;
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        // The painted log stays at the follow-mode tail. Newer lines wait in
        // `pending_logs` until the reader returns to the end, so this frame
        // does not lay out the unread tail or shift the lines on screen.
        if !runtime.verbose_unpinned {
            runtime.accept_followed_logs();
        }
        let scroll = egui::ScrollArea::vertical()
            .id_salt(("cloud-output", id, place))
            .max_height(inner)
            .min_scrolled_height(inner)
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show_viewport(ui, |ui, viewport| paint_log(ui, id, runtime, follows, viewport));
        let max_offset = (scroll.content_size.y - scroll.inner_rect.height()).max(0.0);
        if scroll.state.offset.y + 1.0 < max_offset {
            runtime.unpinned_views |= view_bit(place);
            runtime.verbose_unpinned = true;
        }
    });
}

/// Each place a log is shown keeps its own follow state.
fn view_bit(place: &str) -> u8 {
    match place {
        "body" => 1,
        "drawer" => 2,
        _ => 4,
    }
}

/// Called once per cloud per frame before its output views are drawn: lines are held
/// aside only while a view shown last frame was scrolled up.
pub(super) fn begin_frame(runtime: &mut Runtime) {
    runtime.unpinned_last = std::mem::take(&mut runtime.unpinned_views);
    runtime.verbose_unpinned = runtime.unpinned_last != 0;
    if !runtime.verbose_unpinned {
        // Held lines join now, before newer ones, so the output stays in order.
        runtime.accept_followed_logs();
    }
}

/// The lines a view shows: a scrolled-up view the still list, a following view the
/// newest follow-length tail with held lines included.
fn shown(runtime: &Runtime, follows: bool) -> Vec<&LogLine> {
    if !follows {
        return runtime.logs.iter().collect();
    }
    let total = runtime.logs.len() + runtime.pending_logs.len();
    runtime
        .logs
        .iter()
        .chain(&runtime.pending_logs)
        .skip(total.saturating_sub(Runtime::FOLLOW_LOG_LINES))
        .collect()
}

/// Time column, the gap on either side of it, the stage bar, and the gap before the text.
const LINE_GUTTER: f32 = 38.0 + 8.0 + 2.0 + 8.0;
/// `stage_heading` leads with 2px and allocates a 16px bar.
const STEP_HEADING_HEIGHT: f32 = 18.0;
/// The Notes label is a 12px line under the same 2px lead.
const NOTE_HEADING_HEIGHT: f32 = 22.0;
/// The gap the log used between rows (`item_spacing.y`).
const ROW_GAP: f32 = 3.0;

/// Heights of wrapped rows, moved out of egui memory for the frame and put back.
/// Cloning the map every frame would copy tens of thousands of entries.
#[derive(Clone, Default)]
struct HeightCache {
    entries: HashMap<(u32, u64), f32>,
}

fn height_cache_id(id: u32) -> egui::Id {
    egui::Id::new(("cloud-log-heights", id))
}

/// Drop the wrapped-row height cache for a cloud that has left the board.
/// egui keeps temp data until the context shuts down, so a closed cloud has to
/// remove its own cache or a long-running session retains it.
pub(in crate::app::cloud_panel) fn forget_log_heights(ctx: &egui::Context, id: u32) {
    ctx.data_mut(|data| {
        let _ = data.remove_temp::<HeightCache>(height_cache_id(id));
    });
}

#[cfg(test)]
pub(in crate::app::cloud_panel) fn remember_log_height_cache(ctx: &egui::Context, id: u32) {
    ctx.data_mut(|data| {
        data.insert_temp(
            height_cache_id(id),
            HeightCache {
                entries: HashMap::from([((320, 1), 16.0)]),
            },
        );
    });
}

#[cfg(test)]
pub(in crate::app::cloud_panel) fn log_height_cache_present(ctx: &egui::Context, id: u32) -> bool {
    ctx.data(|data| data.get_temp::<HeightCache>(height_cache_id(id)).is_some())
}

enum Item<'a> {
    Step { stage: Stage, attempt: u64, visit: usize },
    Notes,
    Line(&'a LogLine),
}

struct Placed<'a> {
    index: usize,
    y: f32,
    height: f32,
    item: Item<'a>,
}

/// Paint the rows that intersect `viewport`. The log keeps as many lines as a shell
/// panel, and only the rows in view are laid out.
fn paint_log(ui: &mut egui::Ui, id: u32, runtime: &Runtime, follows: bool, viewport: Rect) {
    let shown = shown(runtime, follows);
    let width = ui.available_width();
    let cache_id = height_cache_id(id);
    let mut cache = ui
        .data_mut(|data| data.remove_temp::<HeightCache>(cache_id))
        .unwrap_or_default();
    if cache.entries.len() > horizon_core::PANEL_SCROLLBACK_LIMIT.saturating_mul(4) {
        cache.entries.clear();
    }
    let plan = layout_plan(ui, &shown, &mut cache, width, viewport);
    ui.data_mut(|data| {
        data.insert_temp(cache_id, cache);
    });
    if shown.is_empty() {
        ui.label(RichText::new("No output yet.").size(TEXT_SIZE).color(theme::FG_DIM()));
        return;
    }
    ui.set_height(plan.total);
    if let Some(first) = plan.rows.first() {
        // Several widgets per row. Skipping by row index keeps a row's ids stable while
        // the reader scrolls, without colliding with the next row's widgets.
        ui.skip_ahead_auto_ids(first.index.saturating_mul(8));
    }
    let origin_y = ui.max_rect().top();
    let left = ui.max_rect().left();
    for placed in &plan.rows {
        let rect = Rect::from_min_size(egui::pos2(left, origin_y + placed.y), Vec2::new(width, placed.height));
        ui.scope_builder(
            UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)),
            |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 0.0;
                match placed.item {
                    Item::Step { stage, attempt, visit } => stage_heading(ui, runtime, stage, attempt, visit),
                    Item::Notes => note_heading(ui),
                    Item::Line(line) => row(ui, line),
                }
            },
        );
    }
}

struct Plan<'a> {
    total: f32,
    rows: Vec<Placed<'a>>,
}

fn layout_plan<'a>(
    ui: &mut egui::Ui,
    shown: &'a [&LogLine],
    cache: &mut HeightCache,
    width: f32,
    viewport: Rect,
) -> Plan<'a> {
    // Floor so a cached wrap is never measured wider than the label. Rounding up
    // can store a line one row short, and the next row then overlaps it.
    let text_width = (width - LINE_GUTTER).max(1.0).floor();
    let width_key = width_key(text_width);
    let top = viewport.min.y - 48.0;
    let bottom = viewport.max.y + 48.0;
    let mut y = 0.0;
    let mut index = 0usize;
    let mut rows = Vec::new();
    let mut any = false;
    let mut previous: Option<(Option<Stage>, u64, usize)> = None;
    for line in shown.iter().copied().filter(|line| !line.text.trim().is_empty()) {
        any = true;
        let group = (line.stage, line.attempt, line.visit);
        if previous != Some(group) {
            if let Some(stage) = line.stage {
                place(
                    &mut rows,
                    &mut y,
                    &mut index,
                    STEP_HEADING_HEIGHT,
                    top,
                    bottom,
                    Item::Step {
                        stage,
                        attempt: line.attempt,
                        visit: line.visit,
                    },
                );
            } else if previous.is_some() {
                place(
                    &mut rows,
                    &mut y,
                    &mut index,
                    NOTE_HEADING_HEIGHT,
                    top,
                    bottom,
                    Item::Notes,
                );
            }
            previous = Some(group);
        }
        let height = line_height(ui, cache, width_key, text_width, line);
        place(&mut rows, &mut y, &mut index, height, top, bottom, Item::Line(line));
    }
    let total = if any { (y - ROW_GAP).max(0.0) } else { 0.0 };
    Plan { total, rows }
}

fn place<'a>(
    rows: &mut Vec<Placed<'a>>,
    y: &mut f32,
    index: &mut usize,
    height: f32,
    top: f32,
    bottom: f32,
    item: Item<'a>,
) {
    let row_y = *y;
    if row_y < bottom && row_y + height > top {
        rows.push(Placed {
            index: *index,
            y: row_y,
            height,
            item,
        });
    }
    *y += height + ROW_GAP;
    *index += 1;
}

fn line_height(ui: &mut egui::Ui, cache: &mut HeightCache, width_key: u32, text_width: f32, line: &LogLine) -> f32 {
    let key = (width_key, line.text_key);
    if let Some(height) = cache.entries.get(&key).copied() {
        return height;
    }
    let measured = ui.fonts_mut(|fonts| {
        fonts
            .layout(
                line.text.clone(),
                FontId::monospace(TEXT_SIZE),
                egui::Color32::GRAY,
                text_width,
            )
            .size()
            .y
    });
    let height = measured.max(TEXT_SIZE + 3.0).ceil();
    cache.entries.insert(key, height);
    height
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn width_key(text_width: f32) -> u32 {
    text_width.clamp(1.0, 16_384.0).round() as u32
}

fn note_heading(ui: &mut egui::Ui) {
    ui.add_space(2.0);
    ui.label(RichText::new("Notes").size(12.0).color(theme::FG_DIM()));
}

/// Only the current attempt's steps are timed from the timeline, each visit with its own
/// time; an earlier attempt's is labelled as such rather than given the current one's.
fn stage_heading(ui: &mut egui::Ui, runtime: &Runtime, stage: Stage, attempt: u64, visit: usize) {
    let color = stage_color(stage);
    let current = attempt == runtime.progress.attempt();
    let duration = runtime.progress.visit_duration(stage, visit).filter(|_| current);
    let label = duration.map_or_else(
        || {
            if current {
                stage.label().to_owned()
            } else {
                format!("{} · earlier attempt", stage.label())
            }
        },
        |elapsed| {
            format!(
                "{} · {}",
                stage.label(),
                horizon_core::cloud_runtime::progress::duration(elapsed)
            )
        },
    );
    ui.add_space(2.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.0), egui::Sense::hover());
    ui.painter().line_segment(
        [rect.left_center(), rect.right_center()],
        Stroke::new(1.0, theme::alpha(color, 50)),
    );
    let galley = ui.painter().layout_no_wrap(label, FontId::proportional(12.0), color);
    let label_rect = egui::Rect::from_min_size(
        rect.left_top() + Vec2::new(52.0, (rect.height() - galley.size().y) / 2.0),
        galley.size() + Vec2::new(8.0, 0.0),
    );
    ui.painter().rect_filled(label_rect, 3, theme::BG());
    ui.painter().galley(label_rect.min + Vec2::new(4.0, 0.0), galley, color);
}

fn row(ui: &mut egui::Ui, line: &LogLine) {
    let failure = line.kind == LineKind::Failure;
    let warning = line.kind == LineKind::Warning;
    let color = if failure {
        theme::PALETTE_RED()
    } else if warning {
        theme::PALETTE_YELLOW()
    } else {
        theme::FG_SOFT()
    };
    let background = ui.painter().add(egui::Shape::Noop);
    let response = ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let time = line.at.map_or_else(String::new, |at| {
            let seconds = at.as_secs();
            format!("{:>2}:{:02}", seconds / 60, seconds % 60)
        });
        ui.add_sized(
            [38.0, TEXT_SIZE + 3.0],
            egui::Label::new(
                RichText::new(time)
                    .monospace()
                    .size(TEXT_SIZE - 1.0)
                    .color(theme::FG_DIM()),
            ),
        );
        let (bar, _) = ui.allocate_exact_size(Vec2::new(2.0, TEXT_SIZE + 3.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(bar, 1, line.stage.map_or(theme::BORDER_SUBTLE(), stage_color));
        ui.add(egui::Label::new(RichText::new(&line.text).monospace().size(TEXT_SIZE).color(color)).wrap());
    });
    if failure {
        ui.painter().set(
            background,
            egui::Shape::rect_filled(
                response.response.rect.expand2(Vec2::new(4.0, 1.0)),
                3,
                theme::alpha(theme::PALETTE_RED(), 28),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::Stage;
    use super::*;
    use crate::test_egui::DiscardTextures;

    #[test]
    fn a_failed_log_gives_its_lines_the_whole_height_it_was_given() {
        let mut runtime = Runtime::default();
        for index in 0..40 {
            runtime
                .logs
                .push_back(LogLine::new(format!("line {index}"), Some(Stage::Push), None));
        }
        runtime.push_log("Error response from daemon: Conflict. ".to_owned() + &"0123456789abcdef".repeat(8));
        let mut used = 0.0;
        let output = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(420.0);
                let top = ui.cursor().top();
                show(ui, 1, "test", &mut runtime, 300.0);
                used = ui.cursor().top() - top;
            })
            .discard_textures();
        assert!(used <= 300.0 + 12.0, "the log fits the given height: {used}");
        let mut lines = 0;
        for shape in &output.shapes {
            // A line without a time draws an empty label, which has no bounds.
            if let egui::Shape::Text(text) = &shape.shape
                && !text.galley.text().is_empty()
            {
                assert!(
                    !text.galley.text().starts_with("Root cause"),
                    "the cause is not repeated"
                );
                assert!(
                    text.visual_bounding_rect().right() <= 420.0 + 1.0,
                    "a long hash wraps inside the card: {}",
                    text.galley.text()
                );
                lines += usize::from(text.galley.text().starts_with("line "));
            }
        }
        assert!(lines >= 8, "several lines stay readable beside a long failure: {lines}");
    }

    fn headings(runtime: &mut Runtime) -> Vec<String> {
        headings_of(runtime, "Push image")
    }

    fn headings_of(runtime: &mut Runtime, step: &str) -> Vec<String> {
        let mut shown = Vec::new();
        let output = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                show(ui, 1, "test", runtime, 400.0);
            })
            .discard_textures();
        for shape in &output.shapes {
            if let egui::Shape::Text(text) = &shape.shape
                && text.galley.text().starts_with(step)
            {
                shown.push(text.galley.text().to_owned());
            }
        }
        shown
    }

    #[test]
    fn a_retried_step_gets_its_own_heading_and_the_old_one_no_current_time() {
        let mut runtime = Runtime {
            stage: Some(Stage::Push),
            ..Runtime::default()
        };
        runtime.progress.stage(Stage::Push, std::time::Instant::now());
        runtime.push_log("error from registry: denied".into());
        runtime.progress.reset();
        runtime.progress.stage(Stage::Push, std::time::Instant::now());
        runtime.push_log("docker push registry.example/worker:tag".into());
        let shown = headings(&mut runtime);
        assert_eq!(shown.len(), 2, "{shown:?}");
        assert_eq!(shown[0], "Push image · earlier attempt");
        assert!(shown[1].starts_with("Push image · 0m"), "{shown:?}");
    }

    #[test]
    fn a_step_revisited_in_one_attempt_keeps_each_visits_own_time() {
        use std::time::{Duration, Instant};
        let start = Instant::now().checked_sub(Duration::from_secs(300)).unwrap();
        let mut runtime = Runtime::default();
        let enter = |runtime: &mut Runtime, stage: Stage, at: Duration, text: &str| {
            runtime.stage = Some(stage);
            runtime.progress.stage(stage, start + at);
            runtime.push_log(text.into());
        };
        enter(&mut runtime, Stage::Validate, Duration::ZERO, "checking the profile");
        enter(&mut runtime, Stage::Build, Duration::from_secs(5), "#1 building");
        enter(
            &mut runtime,
            Stage::Validate,
            Duration::from_secs(125),
            "checking the image contract",
        );
        runtime.progress.finish(start + Duration::from_secs(128));
        let shown = headings_of(&mut runtime, "Validate");
        assert_eq!(
            shown,
            ["Validate · 0m 05s", "Validate · 0m 03s"],
            "each visit keeps its own time"
        );
    }

    #[test]
    fn a_view_still_following_shows_lines_another_scrolled_up_view_holds() {
        let mut runtime = Runtime::default();
        runtime.push_log("first".into());
        runtime.push_log("5f70bf18a086: Pushing [=>   ]".into());
        // The drawer's reader scrolled up last frame; the body is still at the end.
        runtime.unpinned_views = view_bit("drawer");
        begin_frame(&mut runtime);
        runtime.push_log("second".into());
        runtime.push_log("5f70bf18a086: Pushed".into());
        let text = |follows| -> Vec<String> {
            shown(&runtime, follows)
                .into_iter()
                .map(|line| line.text.clone())
                .collect()
        };
        let body = runtime.unpinned_last & view_bit("body") == 0;
        let drawer = runtime.unpinned_last & view_bit("drawer") == 0;
        assert!(body && !drawer);
        assert_eq!(
            text(body),
            ["first", "5f70bf18a086: Pushed", "second"],
            "the following view is current"
        );
        assert_eq!(
            text(drawer),
            ["first", "5f70bf18a086: Pushed"],
            "the scrolled-up view keeps its lines where they are; a layer updates in place"
        );
    }

    #[test]
    fn a_note_after_a_step_gets_its_own_heading_instead_of_the_steps() {
        let mut runtime = Runtime {
            stage: Some(Stage::Sessions),
            ..Runtime::default()
        };
        runtime.progress.stage(Stage::Sessions, std::time::Instant::now());
        runtime.push_log("session started".into());
        runtime.push_note("Idle watch: stopped by claude".into());
        let shown: Vec<String> = {
            let output = egui::Context::default()
                .run_ui(egui::RawInput::default(), |ui| {
                    show(ui, 1, "test", &mut runtime, 400.0);
                })
                .discard_textures();
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect()
        };
        let heading = shown.iter().position(|text| text == "Notes").expect("a Notes heading");
        let note = shown.iter().position(|text| text.contains("Idle watch")).unwrap();
        let step = shown.iter().position(|text| text == "session started").unwrap();
        assert!(step < heading && heading < note, "{shown:?}");
    }

    #[test]
    fn follow_mode_keeps_a_shell_panels_worth_of_lines() {
        let mut runtime = Runtime::default();
        let limit = horizon_core::PANEL_SCROLLBACK_LIMIT;
        for index in 0..=limit {
            runtime.push_log(format!("line-{index}"));
        }
        assert_eq!(runtime.logs.len(), limit);
        assert_eq!(runtime.logs.front().map(|line| line.text.as_str()), Some("line-1"));
        let last = format!("line-{limit}");
        assert_eq!(runtime.logs.back().map(|line| line.text.as_str()), Some(last.as_str()));
    }

    #[test]
    fn a_scrolled_up_log_holds_a_full_history_aside() {
        let mut runtime = Runtime::default();
        runtime.push_log("visible".into());
        runtime.verbose_unpinned = true;
        let limit = horizon_core::PANEL_SCROLLBACK_LIMIT;
        for index in 0..=limit {
            runtime.push_log(format!("burst-{index}"));
        }
        assert_eq!(runtime.logs.len(), 1);
        assert_eq!(runtime.logs.front().map(|line| line.text.as_str()), Some("visible"));
        assert_eq!(runtime.pending_logs.len(), limit);
        assert_eq!(
            runtime.pending_logs.front().map(|line| line.text.as_str()),
            Some("burst-1")
        );
        let last = format!("burst-{limit}");
        assert_eq!(
            runtime.pending_logs.back().map(|line| line.text.as_str()),
            Some(last.as_str())
        );
    }

    #[test]
    fn a_long_log_paints_only_the_visible_tail() {
        let mut runtime = Runtime::default();
        for index in 0..400 {
            runtime.push_log(format!("LOG-LINE-{index:03}"));
        }
        let ctx = egui::Context::default();
        let mut latest = None;
        for frame in 0..6 {
            latest = Some(
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0))),
                        time: Some(f64::from(frame) * 0.05),
                        ..egui::RawInput::default()
                    },
                    |ui| {
                        ui.set_width(640.0);
                        show(ui, 7, "body", &mut runtime, 280.0);
                    },
                )
                .discard_textures(),
            );
        }
        let output = latest.expect("a frame");
        let lines: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text().starts_with("LOG-LINE-") => {
                    Some(text.galley.text().to_owned())
                }
                _ => None,
            })
            .collect();
        assert!(
            lines.len() < 80,
            "painted {} rows, want only the visible tail",
            lines.len()
        );
        assert!(
            lines.iter().any(|line| line == "LOG-LINE-399"),
            "latest line missing: {lines:?}"
        );
        assert!(
            lines.iter().all(|line| line != "LOG-LINE-000"),
            "the tail should hide the first line: {lines:?}"
        );
    }

    #[test]
    fn wrapped_rows_do_not_overlap_and_scrolling_reaches_both_ends() {
        let mut runtime = Runtime::default();
        for index in 0..6 {
            runtime.push_log(format!("WRAP-{index:03} {}", "word ".repeat(6)));
        }
        let ctx = egui::Context::default();
        let mut paint = |events: Vec<egui::Event>, time: f64| {
            let output = ctx
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 500.0))),
                        events,
                        time: Some(time),
                        ..egui::RawInput::default()
                    },
                    |ui| {
                        ui.set_width(200.0);
                        show(ui, 9, "body", &mut runtime, 140.0);
                    },
                )
                .discard_textures();
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text().contains("WRAP-") => {
                        Some((text.galley.text().to_owned(), text.visual_bounding_rect()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let assert_separated = |rows: &[(String, egui::Rect)]| {
            let mut rows = rows.to_vec();
            rows.sort_by(|left, right| left.1.min.y.total_cmp(&right.1.min.y));
            assert!(rows.len() >= 2, "adjacent wrapped rows were not both painted: {rows:?}");
            assert!(
                rows.iter().any(|(_, rect)| rect.height() > TEXT_SIZE * 1.8),
                "a row should wrap onto more than one line: {rows:?}"
            );
            for pair in rows.windows(2) {
                assert!(
                    pair[0].1.max.y <= pair[1].1.min.y + 0.5,
                    "wrapped rows overlap: {pair:?}"
                );
            }
        };

        let mut tail = Vec::new();
        for frame in 0..6 {
            tail = paint(Vec::new(), f64::from(frame) * 0.05);
        }
        assert_separated(&tail);
        assert!(tail.iter().any(|(text, _)| text.contains("WRAP-005")), "{tail:?}");
        assert!(tail.iter().all(|(text, _)| !text.contains("WRAP-000")), "{tail:?}");
        let hover = tail
            .iter()
            .find(|(_, rect)| rect.min.y > 20.0)
            .map_or(egui::pos2(100.0, 60.0), |(_, rect)| rect.center());

        let mut top = Vec::new();
        for frame in 0..24 {
            top = paint(
                vec![
                    egui::Event::PointerMoved(hover),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, 400.0),
                        phase: egui::TouchPhase::Move,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                1.0 + f64::from(frame) * 0.05,
            );
            if top.iter().any(|(text, _)| text.contains("WRAP-000")) {
                break;
            }
        }
        assert_separated(&top);
        assert!(top.iter().any(|(text, _)| text.contains("WRAP-000")), "{top:?}");
        assert!(top.iter().all(|(text, _)| !text.contains("WRAP-005")), "{top:?}");
    }
}

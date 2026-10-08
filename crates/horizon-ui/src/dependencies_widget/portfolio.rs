//! The repository portfolio from the worker's status: health, filters, search, the
//! repository table and the selected repository. Review and changes stay in GitHub.

use egui::{Align, FontId, Layout, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use horizon_core::maintenance::portfolio::{
    self as model, Filter, Repository, ago, clock, keep_selection, reported_health, reported_heartbeat, text,
};
use serde_json::{Map, Value};

use super::{detail, instructions, summary, table, tone, widgets};
use crate::theme;

const HEADER: f32 = 46.0;
const SUMMARY: f32 = 72.0;
const SEARCH: f32 = 40.0;
const FOOTER: f32 = 20.0;
const GAPS: [f32; 4] = [18.0, 14.0, 14.0, 10.0];
const BODY_MIN: f32 = 220.0;
const DETAIL_WIDTH: f32 = 404.0;
/// Below this width the detail replaces the table instead of sitting beside it.
const SPLIT_MIN: f32 = 980.0;
/// Below this width the header uses short action labels.
const ROOMY: f32 = 940.0;

#[derive(Default)]
pub(super) struct State {
    pub(super) search: String,
    pub(super) filter: Filter,
    pub(super) selected: Option<String>,
    pub(super) editor: Option<instructions::Draft>,
    pub(super) save_pending: bool,
    pub(super) save_feedback: Option<Result<(), String>>,
}

impl State {
    pub(super) fn set_save_result(&mut self, result: Result<(), String>) {
        self.save_pending = false;
        if result.is_ok() {
            self.editor = None;
        }
        self.save_feedback = Some(result);
    }
}

pub(super) enum Action {
    OpenTerminal,
    DebugLocalAgent,
    SaveInstructions {
        global: String,
        repositories: Map<String, Value>,
    },
}

pub(super) fn show(
    ui: &mut egui::Ui,
    status: &Value,
    transport_error: Option<&str>,
    state: &mut State,
) -> Option<Action> {
    let mut action = None;
    let fixed = HEADER + SUMMARY + SEARCH + FOOTER + GAPS.iter().sum::<f32>();
    let body = (ui.available_height() - fixed).max(BODY_MIN);
    egui::ScrollArea::vertical()
        .id_salt("dependencies-portfolio")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.push_id("dependencies-portfolio", |ui| {
                portfolio(ui, status, transport_error, state, body, &mut action);
            });
        });
    instructions::sheet(ui.ctx(), state, status).or(action)
}

fn portfolio(
    ui: &mut egui::Ui,
    status: &Value,
    transport_error: Option<&str>,
    state: &mut State,
    body_height: f32,
    action: &mut Option<Action>,
) {
    let repositories = model::repositories(status);
    let counts = model::counts(&repositories);
    let stages = model::pr_stages(status);
    let previous = (state.filter, state.search.clone());
    band(ui, HEADER, GAPS[0], |ui| {
        header(ui, status, state, &repositories, action)
    });
    band(ui, SUMMARY, GAPS[1], |ui| {
        summary::show(ui, state, &counts, repositories.len(), &stages);
    });
    let shown = repositories
        .iter()
        .filter(|repo| repo.matches(state.filter, &state.search))
        .count();
    band(ui, SEARCH, GAPS[2], |ui| {
        if search_row(ui, state, shown, repositories.len()) {
            clear_criteria(state);
        }
    });
    let visible: Vec<_> = repositories
        .iter()
        .filter(|repo| repo.matches(state.filter, &state.search))
        .collect();
    let criteria_changed = previous != (state.filter, state.search.clone());
    if !keep_selection(state.selected.as_deref(), &repositories, &visible, criteria_changed) {
        state.selected = None;
    }
    band(ui, body_height, GAPS[3], |ui| {
        body(ui, &visible, &repositories, status, state, body_height);
    });
    band(ui, FOOTER, 0.0, |ui| footer(ui, status, transport_error, state));
}

/// A fixed-height row of the portfolio followed by `gap`.
fn band(ui: &mut egui::Ui, height: f32, gap: f32, add: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width();
    ui.allocate_ui_with_layout(vec2(width, height), Layout::top_down(Align::Min), |ui| {
        ui.set_min_size(vec2(width, height));
        ui.set_max_height(height);
        add(ui);
    });
    ui.add_space(gap);
}

fn header(
    ui: &mut egui::Ui,
    status: &Value,
    state: &mut State,
    repositories: &[Repository<'_>],
    action: &mut Option<Action>,
) {
    let roomy = ui.available_width() >= ROOMY;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        let (badge, _) = ui.allocate_exact_size(Vec2::splat(HEADER), Sense::hover());
        widgets::dependency_badge(ui.painter(), badge);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                ui.label(RichText::new("Dependencies").size(22.0).strong().color(theme::FG()));
                health(ui, status);
            });
            let prs: usize = repositories.iter().map(|repo| repo.prs.len()).sum();
            let worker = text(status, "worker_id", "");
            let subtitle = format!(
                "{} repositories · {prs} pull requests{}{worker}",
                repositories.len(),
                if worker.is_empty() { "" } else { " · " }
            );
            ui.add(egui::Label::new(RichText::new(subtitle).size(13.0).color(theme::FG_SOFT())).truncate());
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let debug = if roomy { "Debug with local agent" } else { "Debug" };
            if ui
                .add(widgets::chrome_button(debug))
                .on_hover_text("Diagnose the worker from an agent on this computer")
                .clicked()
            {
                *action = Some(Action::DebugLocalAgent);
            }
            let terminal = if roomy { "Worker terminal" } else { "Terminal" };
            if ui
                .add(widgets::chrome_button(terminal))
                .on_hover_text("Follow the worker's run in a terminal panel")
                .clicked()
            {
                *action = Some(Action::OpenTerminal);
            }
            if ui
                .add(widgets::accent_text_button("Instructions"))
                .on_hover_text("Edit global and repository instructions")
                .clicked()
            {
                instructions::open(state, status, None);
            }
        });
    });
}

/// Agent health as a status chip: dot, words and heartbeat age.
fn health(ui: &mut egui::Ui, status: &Value) {
    let health = reported_health(status);
    let color = tone::color(health.tone);
    let mut job = egui::text::LayoutJob::default();
    let format = |color| egui::TextFormat {
        font_id: FontId::proportional(13.0),
        color,
        ..Default::default()
    };
    job.append(health.label, 0.0, format(theme::FG()));
    if let Some(age) = reported_heartbeat(status) {
        job.append(&format!("  ·  {}", ago(age)), 0.0, format(theme::FG_SOFT()));
    }
    let galley = ui.painter().layout_job(job);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + 36.0, 26.0), Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        13,
        theme::alpha(color, 26),
        Stroke::new(1.0, theme::alpha(color, 96)),
        StrokeKind::Inside,
    );
    painter.circle_filled(pos2(rect.left() + 14.0, rect.center().y), 4.0, color);
    painter.galley(
        pos2(rect.left() + 25.0, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::FG(),
    );
    if let Some(error) = status["worker_health"].get("last_error").and_then(Value::as_str) {
        response.on_hover_text(format!("Last error: {error}"));
    }
}

/// Returns true when the person cleared the search and filter.
fn search_row(ui: &mut egui::Ui, state: &mut State, visible: usize, total: usize) -> bool {
    let mut clear = false;
    let width = (ui.available_width() * 0.55).clamp(220.0, 560.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        search_field(ui, &mut state.search, vec2(width, SEARCH));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let filtered = state.filter != Filter::All || !state.search.trim().is_empty();
            if filtered && ui.add(widgets::chrome_button("Clear")).clicked() {
                clear = true;
            }
            let shown = if filtered {
                format!("{visible} of {total} repositories")
            } else {
                format!("{total} repositories")
            };
            ui.label(RichText::new(shown).size(13.0).color(theme::FG_SOFT()));
        });
    });
    clear
}

/// The toolbar search recipe: a well whose border warms toward the accent with attention.
fn search_field(ui: &mut egui::Ui, query: &mut String, size: Vec2) {
    let id = ui.make_persistent_id("dependencies-search");
    let (shell, _) = ui.allocate_exact_size(size, Sense::hover());
    let focused = ui.memory(|memory| memory.has_focus(id));
    let hovered = ui.rect_contains_pointer(shell);
    let warmth = if focused {
        0.78
    } else if hovered {
        0.5
    } else {
        0.32
    };
    let painter = ui.painter().clone();
    if focused {
        painter.rect_stroke(
            shell.expand(2.0),
            12,
            Stroke::new(3.0, theme::alpha(theme::ACCENT(), 36)),
            StrokeKind::Outside,
        );
    }
    painter.rect(
        shell,
        10,
        theme::BG_ELEVATED(),
        Stroke::new(1.0, theme::blend(theme::BORDER_SUBTLE(), theme::ACCENT(), warmth)),
        StrokeKind::Inside,
    );
    widgets::magnifier(
        &painter,
        pos2(shell.left() + 20.0, shell.center().y),
        if focused { theme::ACCENT() } else { theme::FG_SOFT() },
    );
    let clear_width = if query.is_empty() { 12.0 } else { 40.0 };
    let font = FontId::proportional(15.0);
    // One text row tall and centred, so the caret and hint sit on the shell's midline.
    let row = painter
        .layout_no_wrap("Ag".to_owned(), font.clone(), theme::FG())
        .size()
        .y;
    let field = Rect::from_min_max(
        pos2(shell.left() + 38.0, shell.center().y - row / 2.0),
        pos2(shell.right() - clear_width, shell.center().y + row / 2.0),
    );
    ui.put(
        field,
        egui::TextEdit::singleline(query)
            .id(id)
            .hint_text(RichText::new("Search repositories, ecosystems or pull requests").color(theme::FG_DIM()))
            .font(font)
            .text_color(theme::FG())
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .desired_width(field.width()),
    );
    if !query.is_empty() {
        let button = Rect::from_center_size(pos2(shell.right() - 20.0, shell.center().y), Vec2::splat(24.0));
        let response = ui.interact(button, id.with("clear"), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Clear search"));
        if response.hovered() {
            painter.rect_filled(button, 6, theme::PANEL_BG_ALT());
        }
        let color = if response.hovered() {
            theme::FG()
        } else {
            theme::FG_SOFT()
        };
        widgets::cross_mark(&painter, button.center(), 12.0, color);
        if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            query.clear();
        }
    }
}

fn body(
    ui: &mut egui::Ui,
    visible: &[&Repository<'_>],
    repositories: &[Repository<'_>],
    status: &Value,
    state: &mut State,
    height: f32,
) {
    let selected = state
        .selected
        .as_deref()
        .and_then(|selected| repositories.iter().find(|repo| repo.name == selected));
    let Some(repo) = selected else {
        if table::show(ui, visible, state, height) {
            clear_criteria(state);
        }
        return;
    };
    let width = ui.available_width();
    if width < SPLIT_MIN {
        detail_pane(ui, repo, status, state, height);
        return;
    }
    let gutter = 16.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gutter;
        ui.allocate_ui_with_layout(
            vec2(width - DETAIL_WIDTH - gutter, height),
            Layout::top_down(Align::Min),
            |ui| {
                if table::show(ui, visible, state, height) {
                    clear_criteria(state);
                }
            },
        );
        ui.allocate_ui_with_layout(vec2(DETAIL_WIDTH, height), Layout::top_down(Align::Min), |ui| {
            detail_pane(ui, repo, status, state, height);
        });
    });
}

fn detail_pane(ui: &mut egui::Ui, repo: &Repository<'_>, status: &Value, state: &mut State, height: f32) {
    match detail::show(ui, repo, status, height) {
        Some(detail::Request::Close) => state.selected = None,
        Some(detail::Request::EditInstructions) => {
            instructions::open(state, status, Some(repo.name.to_owned()));
        }
        None => {}
    }
}

fn footer(ui: &mut egui::Ui, status: &Value, transport_error: Option<&str>, state: &State) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if let Some(error) = transport_error {
            let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 3.5, theme::PALETTE_YELLOW());
            ui.label(RichText::new(error).size(12.5).color(theme::PALETTE_YELLOW()));
        } else {
            let applied = status
                .get("applied_revision")
                .and_then(Value::as_u64)
                .map_or_else(String::new, |applied| {
                    format!(" · instruction revision {applied} applied")
                });
            let updated = format!("Updated {}{applied}", clock(text(status, "updated_at", "—")));
            ui.label(RichText::new(updated).size(12.5).color(theme::FG_SOFT()));
        }
        if let Some(Ok(())) = &state.save_feedback {
            ui.add_space(8.0);
            let (mark, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            widgets::check_mark(ui.painter(), mark.center(), 10.0, theme::PALETTE_GREEN());
            ui.label(
                RichText::new("Instructions saved to the worker")
                    .size(12.5)
                    .color(theme::PALETTE_GREEN()),
            );
        }
        if status.get("synthetic").and_then(Value::as_bool) == Some(true) {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new("GitHub, CI, review and merge are simulated")
                        .size(12.5)
                        .color(theme::FG_SOFT()),
                );
                widgets::pill(ui, "Test worker", theme::PALETTE_CYAN());
            });
        }
    });
}

fn clear_criteria(state: &mut State) {
    state.filter = Filter::All;
    state.search.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn failed_instruction_sync_preserves_the_local_draft() {
        let snapshot = json!({"global_prompt":"Follow required checks.",
            "repos":[{"repository":"sample/web","prompt":"Use the npm recipe."}]});
        let mut state = State::default();
        instructions::open(&mut state, &snapshot, Some("sample/web".to_owned()));
        state.editor.as_mut().unwrap().global = "New local instructions".to_owned();
        state.save_pending = true;
        state.set_save_result(Err("SSH disconnected".to_owned()));
        assert!(!state.save_pending);
        assert_eq!(state.editor.as_ref().unwrap().global, "New local instructions");
        assert_eq!(snapshot["global_prompt"], "Follow required checks.");
        state.set_save_result(Ok(()));
        assert!(state.editor.is_none());
    }

    #[test]
    fn clearing_restores_every_repository() {
        let mut state = State {
            filter: Filter::Attention,
            search: "web".to_owned(),
            ..State::default()
        };
        clear_criteria(&mut state);
        assert_eq!(state.filter, Filter::All);
        assert!(state.search.is_empty());
    }
}

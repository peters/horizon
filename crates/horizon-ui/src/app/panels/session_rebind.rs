use std::sync::Arc;

use egui::{Button, FontId, RichText, Vec2, text::LayoutJob, text::TextFormat};
use horizon_core::{AgentSessionBinding, PanelId};

const PAGE_SIZE: usize = 8;

use super::session_deletion::{SessionDeletionUi, deletion_progress, queue_deletion_request};
use crate::{text::truncate_chars, theme};

#[derive(Clone)]
struct SessionPicker {
    panel_id: PanelId,
    last_rendered_frame: u64,
    focus_first: bool,
    options: Arc<[AgentSessionBinding]>,
    deletion: SessionDeletionUi,
}

pub(super) fn open_session_picker(response: &egui::Response, panel_id: PanelId, options: Vec<AgentSessionBinding>) {
    let id = picker_id(&response.ctx);
    let frame = response.ctx.cumulative_frame_nr();
    let deletion = SessionDeletionUi::restored(&response.ctx);
    response.ctx.data_mut(|data| {
        data.insert_temp(
            id,
            SessionPicker {
                panel_id,
                last_rendered_frame: frame,
                focus_first: true,
                options: options.into(),
                deletion,
            },
        );
    });
}

pub(super) fn render_session_picker(
    ctx: &egui::Context,
    panel_id: PanelId,
    options: impl Into<Option<Vec<AgentSessionBinding>>>,
) -> Option<AgentSessionBinding> {
    let id = picker_id(ctx);
    let mut state = ctx.data(|data| data.get_temp::<SessionPicker>(id))?;
    if state.panel_id != panel_id {
        return None;
    }
    if let Some(options) = options.into()
        && state.options.as_ref() != options.as_slice()
    {
        let scope_changed = !AgentSessionBinding::same_saved_session_scope(&state.options, &options);
        state.deletion.reconcile_options(&options);
        state.options = options.into();
        state.focus_first |= scope_changed;
    }
    state.last_rendered_frame = ctx.cumulative_frame_nr();
    let result = egui::Modal::new(id)
        .area(egui::Modal::default_area(id).order(egui::Order::Tooltip))
        .frame(
            egui::Frame::popup(&ctx.global_style())
                .corner_radius(16.0)
                .inner_margin(18.0)
                .stroke(egui::Stroke::new(1.0, theme::BORDER_STRONG())),
        )
        .show(ctx, |ui| {
            render_options_with_deletion(ui, &state.options, state.focus_first, &mut state.deletion)
        });
    state.focus_first = false;
    if !result.should_close() && result.inner.binding.is_none() {
        ctx.data_mut(|data| data.insert_temp(id, state));
    } else {
        ctx.data_mut(|data| data.remove::<SessionPicker>(id));
    }
    result.inner.binding
}

pub(super) fn finish_session_deletion(
    ctx: &egui::Context,
    panel_id: PanelId,
    viewport: egui::ViewportId,
    options: Vec<AgentSessionBinding>,
    report: &horizon_core::AgentSessionDeletionReport,
) {
    let id = egui::Id::new(("session_recovery_picker", viewport));
    ctx.data_mut(|data| {
        if let Some(mut state) = data
            .get_temp::<SessionPicker>(id)
            .filter(|state| state.panel_id == panel_id)
        {
            state.options = options.into();
            state.focus_first = true;
            state.deletion.finish(report);
            data.insert_temp(id, state);
        }
    });
}

fn picker_id(ctx: &egui::Context) -> egui::Id {
    egui::Id::new(("session_recovery_picker", ctx.viewport_id()))
}

pub(in crate::app) fn session_picker_panel(ctx: &egui::Context) -> Option<PanelId> {
    session_picker_panel_in_viewport(ctx, ctx.viewport_id())
}

pub(in crate::app) fn focused_session_picker_panel(ctx: &egui::Context) -> Option<PanelId> {
    let viewport = ctx
        .input(|input| {
            input
                .raw
                .viewports
                .iter()
                .find_map(|(&id, info)| (id != egui::ViewportId::ROOT && info.focused == Some(true)).then_some(id))
        })
        .unwrap_or_else(|| ctx.viewport_id());
    session_picker_panel_in_viewport(ctx, viewport)
}

fn session_picker_panel_in_viewport(ctx: &egui::Context, viewport: egui::ViewportId) -> Option<PanelId> {
    let id = egui::Id::new(("session_recovery_picker", viewport));
    let state = ctx.data(|data| data.get_temp::<SessionPicker>(id))?;
    let frame = ctx.cumulative_frame_nr_for(viewport);
    if frame > state.last_rendered_frame.saturating_add(1) {
        ctx.data_mut(|data| data.remove::<SessionPicker>(id));
        return None;
    }
    Some(state.panel_id)
}

#[derive(Default)]
pub(super) struct SessionRebindRenderOutcome {
    pub(super) binding: Option<AgentSessionBinding>,
    #[cfg(test)]
    pub(super) option_rects: Vec<egui::Rect>,
    #[cfg(test)]
    pub(super) copy_rects: Vec<egui::Rect>,
}

#[cfg(test)]
pub(super) fn render_session_rebind_options(
    ui: &mut egui::Ui,
    rebind_options: &[AgentSessionBinding],
) -> SessionRebindRenderOutcome {
    render_options(ui, rebind_options, false)
}

#[cfg(test)]
fn render_options(
    ui: &mut egui::Ui,
    rebind_options: &[AgentSessionBinding],
    focus_first: bool,
) -> SessionRebindRenderOutcome {
    render_options_with_deletion(ui, rebind_options, focus_first, &mut SessionDeletionUi::default())
}

fn render_options_with_deletion(
    ui: &mut egui::Ui,
    rebind_options: &[AgentSessionBinding],
    focus_first: bool,
    deletion: &mut SessionDeletionUi,
) -> SessionRebindRenderOutcome {
    let page_id = ui.make_persistent_id("session_page");
    let reset_id = ui.make_persistent_id("session_page_reset");
    let reset_scroll = focus_first || ui.data(|data| data.get_temp::<bool>(reset_id).unwrap_or_default());
    let mut reset_next = false;
    let mut page = ui.data(|data| data.get_temp::<usize>(page_id).unwrap_or_default());
    page = if focus_first {
        0
    } else {
        page.min(rebind_options.len().saturating_sub(1) / PAGE_SIZE)
    };
    let mut outcome = SessionRebindRenderOutcome::default();
    let width = (ui.ctx().content_rect().width() - 40.0).clamp(280.0, 540.0);
    ui.set_width(width);
    ui.spacing_mut().button_padding = Vec2::new(12.0, 8.0);
    ui.spacing_mut().scroll.floating = false;
    ui.visuals_mut().widgets.inactive.weak_bg_fill = theme::PANEL_BG_ALT();
    let content_start = ui.cursor().top();
    render_session_header(ui, rebind_options.len());
    if let Some((done, total)) = deletion_progress(ui.ctx()) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(format!("Deleting saved conversations… {done}/{total}"));
        });
        return outcome;
    }
    if deletion.confirming() {
        if let Some(sessions) = deletion.render_confirmation(ui) {
            queue_deletion_request(ui.ctx(), sessions);
        }
        return outcome;
    }
    deletion.render_toolbar(ui, rebind_options);
    if rebind_options.is_empty() {
        ui.label("No saved conversations remain in this list.");
    }
    let mut scroll = egui::ScrollArea::vertical()
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .max_height(
            (ui.ctx().content_rect().height() - (ui.cursor().top() - content_start) - 160.0).clamp(100.0, 480.0),
        );
    if reset_scroll {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    scroll.show(ui, |ui| {
        for (index, binding) in rebind_options.iter().skip(page * PAGE_SIZE).take(PAGE_SIZE).enumerate() {
            render_session_row(ui, binding, focus_first && index == 0, deletion, &mut outcome);
            if outcome.binding.is_some() {
                break;
            }
            ui.add_space(8.0);
        }
    });
    reset_next |= render_session_pagination(ui, rebind_options.len(), &mut page);
    ui.data_mut(|data| {
        data.insert_temp(page_id, page);
        data.insert_temp(reset_id, reset_next);
    });
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(4.0);
    ui.label(
        RichText::new("Selecting a session restarts this panel.")
            .size(12.0)
            .color(theme::FG_SOFT()),
    );
    outcome
}

fn render_session_pagination(ui: &mut egui::Ui, count: usize, page: &mut usize) -> bool {
    let mut changed = false;
    if count > PAGE_SIZE {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(*page > 0, Button::new("Previous")).clicked() {
                *page -= 1;
                changed = true;
            }
            ui.label(format!(
                "{}–{} of {}",
                *page * PAGE_SIZE + 1,
                ((*page + 1) * PAGE_SIZE).min(count),
                count
            ));
            if ui
                .add_enabled((*page + 1) * PAGE_SIZE < count, Button::new("Next"))
                .clicked()
            {
                *page += 1;
                changed = true;
            }
        });
    }

    changed
}

fn render_session_header(ui: &mut egui::Ui, count: usize) {
    ui.horizontal(|ui| {
        egui::Frame::new()
            .fill(theme::alpha(theme::ACCENT(), 24))
            .corner_radius(12.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::hover());
                let center = rect.center();
                let stroke = egui::Stroke::new(1.8, theme::ACCENT());
                ui.painter().circle_stroke(center, 10.0, stroke);
                ui.painter()
                    .line_segment([center, center - Vec2::new(0.0, 6.0)], stroke);
                ui.painter()
                    .line_segment([center, center + Vec2::new(5.0, 3.0)], stroke);
            });
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.label(RichText::new("Resume a session").size(20.0).strong().color(theme::FG()));
            ui.label(
                RichText::new(format!(
                    "{} {} available · Newest first",
                    count,
                    if count == 1 { "session" } else { "sessions" }
                ))
                .size(12.0)
                .color(theme::FG_SOFT()),
            );
        });
    });
    ui.add_space(16.0);
}

fn session_row_text(binding: &AgentSessionBinding, label: &str) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        &truncate_chars(label, 60),
        0.0,
        TextFormat {
            font_id: FontId::proportional(15.0),
            line_height: Some(22.0),
            color: theme::FG(),
            ..Default::default()
        },
    );
    job.append(
        &format!("\n{}", binding.last_used_display()),
        0.0,
        TextFormat {
            font_id: FontId::proportional(12.0),
            line_height: Some(20.0),
            color: theme::FG_SOFT(),
            ..Default::default()
        },
    );
    job.append(
        &format!("\n{}", binding.session_id),
        0.0,
        TextFormat {
            font_id: FontId::monospace(12.0),
            line_height: Some(18.0),
            color: theme::FG_SOFT(),
            ..Default::default()
        },
    );
    job
}

fn render_session_row(
    ui: &mut egui::Ui,
    binding: &AgentSessionBinding,
    focus_first: bool,
    deletion: &mut SessionDeletionUi,
    outcome: &mut SessionRebindRenderOutcome,
) {
    let label = binding
        .label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| binding.kind.display_name());
    let mut job = session_row_text(binding, label);
    ui.push_id((&binding.kind, &binding.session_id), |ui| {
        let mut focused = false;
        let card = egui::Frame::new()
            .fill(theme::PANEL_BG_ALT())
            .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE()))
            .corner_radius(12.0)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 84.0).max(1.0);
                    let padding = ui.spacing().button_padding * 2.0;
                    job.wrap.max_width = (width - padding.x).max(1.0);
                    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                    let height = (galley.size().y + padding.y).max(78.0);
                    let response = ui.add_sized(
                        Vec2::new(width, height),
                        Button::new(galley).right_text(()).frame(false),
                    );
                    if focus_first {
                        response.request_focus();
                    }
                    focused = response.has_focus();
                    #[cfg(test)]
                    outcome.option_rects.push(response.rect);
                    if response.clicked() {
                        outcome.binding = Some(binding.clone());
                        ui.close();
                    }
                    response.on_hover_text(format!(
                        "{label}\nSession ID: {}\nClick to resume in this panel.",
                        binding.session_id
                    ));
                    ui.vertical(|ui| {
                        let copy = ui.add(
                            Button::new(RichText::new("Copy ID").size(12.0).color(theme::FG()))
                                .fill(theme::alpha(theme::ACCENT(), 20))
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(8.0),
                        );
                        copy.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                copy.enabled(),
                                format!("Copy conversation ID {}", binding.session_id),
                            )
                        });
                        focused |= copy.has_focus();
                        #[cfg(test)]
                        outcome.copy_rects.push(copy.rect);
                        if copy.clicked() {
                            ui.ctx().copy_text(binding.session_id.clone());
                        }
                        copy.on_hover_text("Copy the full session ID");
                        deletion.render_row_controls(ui, binding);
                    });
                })
            });
        if focused || card.response.contains_pointer() {
            ui.painter().rect_stroke(
                card.response.rect,
                12.0,
                egui::Stroke::new(1.0, theme::ACCENT()),
                egui::StrokeKind::Inside,
            );
        }
    });
}

#[cfg(test)]
mod tests;

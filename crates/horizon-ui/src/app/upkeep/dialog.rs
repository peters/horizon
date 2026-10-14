//! The New Upkeep dialog: which repositories, which bots, how often, how the worker checks
//! out code, and the instructions every run starts from.
use egui::{
    Align, Context, CornerRadius, CursorIcon, Frame, Id, Layout, Margin, Order, RichText, Sense, Stroke, StrokeKind,
    TextEdit, Ui, Vec2,
};
use horizon_core::upkeep::DEFAULT_INSTRUCTIONS;
use horizon_core::upkeep::worker::Checkouts;

use super::form::{Bot, Every, Form, checkouts_label};
use crate::theme;

const SUMMARY_WIDTH: f32 = 330.0;
const GUTTER: f32 = 24.0;
const TWO_COLUMNS: f32 = 800.0;

pub(super) enum Outcome {
    Open,
    Cancel,
    Start,
}

pub(super) fn show(ctx: &Context, form: &mut Form) -> Outcome {
    let viewport = ctx.content_rect();
    let width = (viewport.width() - 64.0).clamp(240.0, 1120.0);
    let body_height = (viewport.height() - 250.0).clamp(280.0, 660.0);
    let mut outcome = Outcome::Open;
    let id = Id::new("upkeep-creation");
    let response = egui::Modal::new(id)
        .area(egui::Modal::default_area(id).order(Order::Tooltip))
        .frame(
            Frame::new()
                .fill(theme::BG_ELEVATED())
                .stroke(Stroke::new(1.0, theme::BORDER_STRONG()))
                .corner_radius(16)
                .inner_margin(24),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = Vec2::new(10.0, 8.0);
            ui.label(RichText::new("New Upkeep").size(26.0).strong().color(theme::FG()));
            ui.label(
                RichText::new(
                    "One agent on a cloud worker reviews, fixes and merges dependency pull requests on a schedule.",
                )
                .size(13.5)
                .color(theme::FG_SOFT()),
            );
            ui.add_space(16.0);
            if form.proposed {
                proposal(ui, form);
            }
            ui.add_enabled_ui(!form.starting, |ui| {
                if width >= TWO_COLUMNS {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = GUTTER;
                        column(ui, width - SUMMARY_WIDTH - GUTTER, body_height, |ui| {
                            scroll(ui, "upkeep-body", body_height, |ui| body(ui, form));
                        });
                        column(ui, SUMMARY_WIDTH, body_height, |ui| summary(ui, form));
                    });
                } else {
                    scroll(ui, "upkeep-body", body_height, |ui| {
                        body(ui, form);
                        ui.add_space(12.0);
                        summary(ui, form);
                    });
                }
            });
            ui.add_space(12.0);
            if let Some(chosen) = footer(ui, form) {
                outcome = chosen;
            }
        });
    ctx.move_to_top(response.response.layer_id);
    if response.should_close() && matches!(outcome, Outcome::Open) {
        outcome = Outcome::Cancel;
    }
    outcome
}

/// An agent filled the form in: the person checks it before Start, the instructions above
/// all, because they become the prompt of an agent with push access.
fn proposal(ui: &mut Ui, form: &Form) {
    let text = if form.instructions == DEFAULT_INSTRUCTIONS {
        "An agent proposed these settings. Check each repository before you start."
    } else {
        "An agent proposed these settings and its own instructions. Read the instructions in full before you start."
    };
    Frame::new()
        .fill(theme::alpha(theme::ACCENT(), 24))
        .corner_radius(10)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).size(13.0).color(theme::FG()));
        });
    ui.add_space(10.0);
}

fn scroll(ui: &mut Ui, id: &str, height: f32, add: impl FnOnce(&mut Ui)) {
    let output = egui::ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, true])
        .max_height(height)
        .show(ui, |ui| {
            ui.set_width(ui.available_width() - 12.0);
            add(ui);
            ui.add_space(8.0);
        });
    // Content below the fold fades out instead of ending in a cut-off row.
    let hidden = output.content_size.y - output.state.offset.y - output.inner_rect.height();
    if hidden > 1.0 {
        let rect = output.inner_rect;
        let fade = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - 28.0), rect.max);
        let painter = ui.painter_at(fade);
        for step in 0..14_u8 {
            let t = f32::from(step) / 14.0;
            let band = egui::Rect::from_min_max(
                egui::pos2(fade.min.x, fade.min.y + t * 28.0),
                egui::pos2(fade.max.x, fade.min.y + (t + 1.0 / 14.0) * 28.0),
            );
            let opacity = (t * 230.0).clamp(0.0, 255.0);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "clamped to 0..=255"
            )]
            painter.rect_filled(band, 0, theme::alpha(theme::BG_ELEVATED(), opacity as u8));
        }
    }
}

fn column(ui: &mut Ui, width: f32, height: f32, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(Vec2::new(width, height), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        add(ui);
    });
}

fn body(ui: &mut Ui, form: &mut Form) {
    field_label(
        ui,
        "Repositories",
        "Upkeep watches pull requests from the bots below in these repositories.",
    );
    super::repositories::show(ui, form);
    ui.add_space(14.0);

    field_label(ui, "Bots", "Pull requests these bots open. Upkeep never replaces them.");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for bot in Bot::ALL {
            let selected = form.bots.contains(&bot);
            if choice(ui, selected, bot.label()).clicked() {
                if selected {
                    form.bots.remove(&bot);
                } else {
                    form.bots.insert(bot);
                }
            }
        }
    });
    ui.add_space(14.0);

    field_label(
        ui,
        "Schedule",
        "Only one run at a time. A run that is still busy skips the next one.",
    );
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for every in Every::ALL {
            if choice(ui, form.every == every, every.label()).clicked() {
                form.every = every;
            }
        }
    });
    ui.add_space(14.0);

    field_label(ui, "Checkouts", "How the worker finds pull requests and gets the code.");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for checkouts in Checkouts::ALL {
            if choice(ui, form.checkouts == checkouts, checkouts_label(checkouts)).clicked() {
                form.checkouts = checkouts;
            }
        }
    });
    hint(
        ui,
        match form.checkouts {
            Checkouts::OnDemand => {
                "GitHub search finds the pull requests in seconds. A repository is cloned, partially, only when one of its pull requests needs a fix. Uses the least time and disk."
            }
            Checkouts::Partial => {
                "Every repository is cloned once without old file contents and updated each run. More disk; fixes start a little sooner."
            }
            Checkouts::Full => {
                "Every repository is cloned in full, with Git LFS. Can take hours and many gigabytes for many repositories; choose it only when builds need the full history."
            }
        },
    );
    ui.add_space(14.0);

    ui.horizontal(|ui| {
        ui.label(RichText::new("Instructions").size(14.0).strong().color(theme::FG()));
        if form.instructions != DEFAULT_INSTRUCTIONS {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if link(ui, "Reset to default").clicked() {
                    form.instructions = DEFAULT_INSTRUCTIONS.to_string();
                }
            });
        }
    });
    ui.add(
        TextEdit::multiline(&mut form.instructions)
            .id(Id::new("upkeep-instructions"))
            .font(egui::FontId::monospace(12.0))
            .margin(Vec2::new(12.0, 10.0))
            .desired_rows(9)
            .desired_width(f32::INFINITY),
    );
    hint(
        ui,
        "Every run starts from these. Each repository's AGENTS.md still applies on top.",
    );
}

fn summary(ui: &mut Ui, form: &Form) {
    Frame::new()
        .fill(theme::PANEL_BG())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(10)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            crate::upkeep_widget::caption(ui, "Summary");
            let (headline, color) = match form.selected.len() {
                0 => ("No repositories yet".to_string(), theme::FG_SOFT()),
                1 => ("1 repository".to_string(), theme::FG()),
                count => (format!("{count} repositories"), theme::FG()),
            };
            ui.label(RichText::new(headline).size(22.0).strong().color(color));
            let open = form.catalog.open_in(&form.selected);
            let counted = form.find_error.is_none() && form.catalog.search_error.is_none();
            ui.label(
                RichText::new(match open {
                    _ if !counted => "Open pull requests could not be counted".to_string(),
                    0 => "No open pull requests right now".to_string(),
                    1 => "1 open pull request to move now".to_string(),
                    open => format!("{open} open pull requests to move now"),
                })
                .size(12.5)
                .color(theme::FG_SOFT()),
            );
            ui.add_space(6.0);
            let bots: Vec<_> = form.bots.iter().map(|bot| bot.label()).collect();
            let bots = if bots.is_empty() {
                "None".to_string()
            } else {
                bots.join(", ")
            };
            fact(ui, "Agent", "Codex");
            fact(ui, "Bots", &bots);
            fact(ui, "Runs", form.every.label());
            fact(ui, "Checkouts", checkouts_label(form.checkouts));
            fact(ui, "Worker", "Hetzner, always on");
            if form.codex_login == Some(false) {
                ui.add(
                    egui::Label::new(
                        RichText::new("Sign in with ChatGPT in Cloud settings first, so Codex can run on the worker.")
                            .size(12.0)
                            .color(theme::PALETTE_YELLOW()),
                    )
                    .wrap(),
                );
            }
            ui.add_space(8.0);
            Frame::new()
                .fill(theme::alpha(theme::ACCENT(), 24))
                .stroke(Stroke::new(1.0, theme::alpha(theme::ACCENT(), 90)))
                .corner_radius(8)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new("Merges on its own")
                            .size(13.5)
                            .strong()
                            .color(theme::ACCENT()),
                    );
                    ui.label(
                        RichText::new(
                            "A pull request merges only when every check and required review passes for its \
                             latest commit. Anything else waits for you on the board.",
                        )
                        .size(12.5)
                        .color(theme::FG_SOFT()),
                    );
                });
            ui.add_space(4.0);
            hint(
                ui,
                "The worker keeps running while this computer sleeps, and costs money until you remove the cloud.",
            );
        });
}

fn fact(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(Vec2::new(72.0, 18.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_width(72.0);
            ui.add(egui::Label::new(RichText::new(label).size(12.5).color(theme::FG_SOFT())).selectable(false));
        });
        ui.add(egui::Label::new(RichText::new(value).size(12.5).color(theme::FG())).truncate());
    });
}

fn footer(ui: &mut Ui, form: &Form) -> Option<Outcome> {
    let mut outcome = None;
    let blocked = form.blocked();
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let start = egui::Button::new(RichText::new("Start Upkeep").size(14.0).strong().color(theme::BG()))
            .fill(theme::ACCENT())
            .corner_radius(10)
            .min_size(Vec2::new(148.0, 40.0));
        let reason = blocked.as_deref().unwrap_or_default();
        if ui
            .add_enabled(blocked.is_none(), start)
            .on_disabled_hover_text(reason)
            .clicked()
        {
            outcome = Some(Outcome::Start);
        }
        let cancel = egui::Button::new(RichText::new("Cancel").size(14.0))
            .corner_radius(10)
            .min_size(Vec2::new(120.0, 40.0));
        if ui.add(cancel).clicked() {
            outcome = Some(Outcome::Cancel);
        }
        if form.starting {
            ui.add(egui::Spinner::new().size(14.0).color(theme::ACCENT()));
        }
        // The reason the start failed outranks why it cannot start again yet.
        if let Some(error) = &form.error {
            ui.add(egui::Label::new(RichText::new(error).size(12.5).color(theme::PALETTE_RED())).truncate())
                .on_hover_text(error);
        } else if blocked.is_some() {
            ui.label(RichText::new(reason).size(12.5).color(theme::FG_SOFT()));
        }
    });
    outcome
}

fn field_label(ui: &mut Ui, label: &str, hint_text: &str) {
    ui.label(RichText::new(label).size(14.0).strong().color(theme::FG()));
    hint(ui, hint_text);
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(theme::FG_SOFT()));
}

pub(super) fn link(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).size(12.0).color(theme::ACCENT())).frame(false))
        .on_hover_cursor(CursorIcon::PointingHand)
}

/// A choice chip from the design guidelines.
fn choice(ui: &mut Ui, selected: bool, label: &str) -> egui::Response {
    let (fill, stroke) = if selected {
        (
            theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.22),
            Stroke::new(1.0, theme::ACCENT()),
        )
    } else {
        (theme::PANEL_BG_ALT(), Stroke::new(1.0, theme::BORDER_SUBTLE()))
    };
    let frame = Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(8)
        .inner_margin(Margin::symmetric(12, 7))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(label).size(12.5).color(theme::FG())).selectable(false));
        });
    let response = ui
        .interact(
            frame.response.rect,
            ui.id().with(("upkeep-choice", label)),
            Sense::click(),
        )
        .on_hover_cursor(CursorIcon::PointingHand);
    if response.hovered() && !selected {
        ui.painter().rect_stroke(
            response.rect,
            CornerRadius::same(8),
            Stroke::new(1.0, theme::FG_DIM()),
            StrokeKind::Inside,
        );
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label));
    response
}

/// A segment of the settings tab bar pattern.
pub(super) fn segment(ui: &mut Ui, selected: bool, label: &str) -> egui::Response {
    let (fill, stroke, color) = if selected {
        (
            theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.20),
            Stroke::new(1.0, theme::blend(theme::BORDER_SUBTLE(), theme::ACCENT(), 0.5)),
            theme::FG(),
        )
    } else {
        (egui::Color32::TRANSPARENT, Stroke::NONE, theme::FG_SOFT())
    };
    ui.add(
        egui::Button::new(RichText::new(label).size(12.5).color(color))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(8)
            .min_size(Vec2::new(0.0, 30.0)),
    )
    .on_hover_cursor(CursorIcon::PointingHand)
}

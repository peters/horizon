//! The two ways to start a repository without cloud settings, as cards side by side:
//! each with a painted mark, what it gives in three short lines, and its one action.
use super::{Choice, Mode, Production, prompt, setup_agent};
use crate::theme;
use egui::{Color32, Frame, Id, Painter, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use horizon_core::cloud_runtime::repository::launch::quick_start;

/// Side by side the cards' content is this tall at least.
const CARD_HEIGHT: f32 = 190.0;

/// The cards stand side by side from this width; each line then fits its column.
const SIDE_BY_SIDE: f32 = 640.0;

fn card_id(key: &str) -> Id {
    Id::unique(("cloud-setup-card", key))
}

pub(super) fn unconfigured(ui: &mut Ui, form: &mut Production) -> Choice {
    let mut choice = Choice::None;
    ui.label(
        RichText::new("No cloud setup yet")
            .size(16.0)
            .strong()
            .color(theme::FG()),
    );
    ui.label(
        RichText::new("This repository has no .horizon/cloud.yml. Start right away, or let an agent build a worker that matches your CI.")
            .size(12.5)
            .color(theme::FG_DIM()),
    );
    ui.add_space(6.0);
    // Side by side when both cards fit with their buttons; stacked in a narrow dialog.
    if ui.available_width() >= SIDE_BY_SIDE {
        ui.columns(2, |columns| {
            quick_start_card(&mut columns[0], &mut choice, true);
            setup_card(&mut columns[1], form, &mut choice, true);
        });
    } else {
        quick_start_card(ui, &mut choice, false);
        ui.add_space(10.0);
        setup_card(ui, form, &mut choice, false);
    }
    choice
}

/// What the built-in quick start worker has, read from its own profile.
pub(super) fn quick_start_facts() -> &'static [String; 3] {
    static FACTS: std::sync::OnceLock<[String; 3]> = std::sync::OnceLock::new();
    FACTS.get_or_init(read_quick_start_facts)
}

fn read_quick_start_facts() -> [String; 3] {
    let described = quick_start::builtin().ok().and_then(|config| {
        let profile = config.profiles.get(quick_start::PROFILE)?.clone();
        let mut tools: Vec<String> = profile
            .capabilities
            .agents
            .iter()
            .map(|agent| capitalized(agent.as_str()))
            .chain(
                profile
                    .capabilities
                    .browsers
                    .iter()
                    .map(|browser| capitalized(browser.as_str())),
            )
            .collect();
        if profile.capabilities.desktop {
            tools.push("a desktop".into());
        }
        let provider = match profile.provider.as_str() {
            "runpod" => "RunPod".to_owned(),
            other => capitalized(other),
        };
        Some([
            format!("{} vCPU · {} GB worker on {provider}", profile.cpu, profile.memory_gb),
            list(&tools),
            "No registry login, image build or Docker here".to_owned(),
        ])
    });
    described.unwrap_or_else(|| {
        [
            "A small worker on the public base image".into(),
            "Claude, Codex, Chromium and a desktop".into(),
            "No registry login, image build or Docker here".into(),
        ]
    })
}

fn capitalized(word: &str) -> String {
    let mut letters = word.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

/// `a, b and c`.
fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn quick_start_card(ui: &mut Ui, choice: &mut Choice, paired: bool) {
    let facts = quick_start_facts();
    card(ui, "quick-start", theme::PALETTE_CYAN(), paired, |ui, tint| {
        header(ui, tint, bolt_mark, "Quick start", "No build needed");
        ui.add_space(4.0);
        for fact in facts {
            fact_row(ui, tint, fact);
        }
        // Both cards say three things, so their buttons share a line.
        ui.add_space(10.0);
        let button = egui::Button::new(RichText::new("Start on the base image").size(14.0).strong().color(tint))
            .fill(theme::alpha(tint, 22))
            .stroke(Stroke::new(1.0, theme::alpha(tint, 150)))
            .corner_radius(10)
            .min_size(Vec2::new(ui.available_width(), 40.0));
        // A centered layout centers the label in the wide button.
        let clicked = ui
            .with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.add(button).clicked()
            })
            .inner;
        if clicked {
            *choice = Choice::QuickStart;
        }
    });
}

fn setup_card(ui: &mut Ui, form: &mut Production, choice: &mut Choice, paired: bool) {
    // Files an earlier setup left uncommitted are where the agent continues.
    let earlier = prompt::setup_files_seen(ui.ctx(), form.repository.trim());
    card(ui, "setup-from-ci", theme::ACCENT(), paired, |ui, tint| {
        let badge = if earlier {
            "Continues your files"
        } else {
            "Matches your CI"
        };
        header(ui, tint, pipeline_mark, "Set up from CI", badge);
        ui.add_space(4.0);
        let facts: [&str; 3] = if earlier {
            [
                "Continues from the uncommitted files in .horizon",
                "Builds the worker image from them",
                "Runs your CI jobs in it to prove it",
            ]
        } else {
            [
                "An agent reads your CI workflows",
                "Writes .horizon and builds the worker image",
                "Runs your CI jobs in it to prove it",
            ]
        };
        for fact in facts {
            fact_row(ui, tint, fact);
        }
        ui.add_space(10.0);
        let verb = if earlier { "Continue with" } else { "Set up with" };
        if setup_agent(ui, form, verb, true) == Choice::SetupAgent {
            form.setup_mode = Mode::New;
            *choice = Choice::SetupAgent;
        }
    });
}

/// A card tinted by `tint` that lights up under the pointer. A `paired` card is as tall
/// as the one beside it.
fn card(ui: &mut Ui, key: &str, tint: Color32, paired: bool, add: impl FnOnce(&mut Ui, Color32)) {
    let id = card_id(key);
    let hovered = ui
        .data(|data| data.get_temp::<Rect>(id))
        .is_some_and(|rect| ui.rect_contains_pointer(rect));
    let strength = ui.ctx().animate_bool_with_time(id, hovered, 0.12);
    let fill = theme::blend(theme::PANEL_BG_ALT(), tint, 0.035 + 0.035 * strength);
    let border = theme::blend(theme::BORDER_SUBTLE(), tint, 0.25 + 0.45 * strength);
    let shown = Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(14)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // The content's own height, measured before any minimum, is what the other card
            // matches: a height copied back and forth would never shrink again.
            let content = ui
                .with_layout(egui::Layout::top_down(egui::Align::Min), |ui| add(ui, tint))
                .response
                .rect
                .height();
            let stored = card_id(key).with("content");
            if ui
                .data(|data| data.get_temp::<f32>(stored))
                .is_none_or(|before| (before - content).abs() > 0.5)
            {
                ui.data_mut(|data| data.insert_temp(stored, content));
                ui.ctx().request_repaint();
            }
            // As tall as the other card's content, so the pair lines up at the bottom too.
            if paired {
                let other = if key == "quick-start" {
                    "setup-from-ci"
                } else {
                    "quick-start"
                };
                let beside = ui.data(|data| data.get_temp::<f32>(card_id(other).with("content")));
                let target = CARD_HEIGHT.max(beside.unwrap_or_default());
                // Exactly that tall from the card's top, whatever spacing follows the content.
                let top = ui.min_rect().min;
                ui.expand_to_include_rect(Rect::from_min_size(top, Vec2::new(0.0, target)));
            }
        });
    let rect = shown.response.rect;
    ui.data_mut(|data| data.insert_temp(id, rect));
    // A soft line of the tint along the top edge.
    let top = Rect::from_min_max(
        rect.left_top() + Vec2::new(16.0, 0.0),
        Pos2::new(rect.right() - 16.0, rect.top() + 2.0),
    );
    ui.painter()
        .rect_filled(top, 1, tint.gamma_multiply(0.35 + 0.45 * strength));
}

/// The mark, the title and a small pill on the right, which steps aside when the card is
/// too narrow for it.
fn header(ui: &mut Ui, tint: Color32, mark: fn(&Painter, Rect, Color32), title: &str, pill: &str) {
    // The card's own width: the row lays out within it, whatever the window around it.
    let width = ui.available_width();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let left = ui.min_rect().left();
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
        ui.painter().rect_filled(rect, 9, theme::alpha(tint, 34));
        ui.painter()
            .rect_stroke(rect, 9, Stroke::new(1.0, theme::alpha(tint, 90)), StrokeKind::Inside);
        mark(ui.painter(), rect.shrink(7.0), tint);
        ui.add(egui::Label::new(RichText::new(title).size(16.0).strong().color(theme::FG())).truncate());
        let text = ui.fonts_mut(|fonts| fonts.layout_no_wrap(pill.to_owned(), egui::FontId::proportional(11.0), tint));
        let pill_width = text.size().x + 16.0;
        let room = width - (ui.min_rect().right() - left) - ui.spacing().item_spacing.x;
        if room < pill_width {
            return;
        }
        ui.add_space(room - pill_width);
        Frame::new()
            .fill(theme::alpha(tint, 26))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.label(RichText::new(pill).size(11.0).strong().color(tint));
            });
    });
}

/// One thing the choice gives, behind a small check.
fn fact_row(ui: &mut Ui, tint: Color32, text: &str) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(16.0, 16.0), Sense::hover());
        let c = rect.center();
        let stroke = Stroke::new(1.6, tint);
        ui.painter()
            .line_segment([c + Vec2::new(-4.0, 0.0), c + Vec2::new(-1.0, 3.0)], stroke);
        ui.painter()
            .line_segment([c + Vec2::new(-1.0, 3.0), c + Vec2::new(4.5, -3.5)], stroke);
        // A narrow card wraps the line instead of running past its edge.
        ui.add(egui::Label::new(RichText::new(text).size(13.0).color(theme::FG_SOFT())).wrap());
    });
}

/// A lightning bolt: start now.
fn bolt_mark(painter: &Painter, rect: Rect, color: Color32) {
    let at = |x: f32, y: f32| Pos2::new(rect.left() + rect.width() * x, rect.top() + rect.height() * y);
    let points = vec![
        at(0.62, 0.0),
        at(0.18, 0.56),
        at(0.48, 0.56),
        at(0.36, 1.0),
        at(0.84, 0.40),
        at(0.54, 0.40),
    ];
    painter.add(egui::Shape::convex_polygon(
        points.clone(),
        theme::alpha(color, 60),
        Stroke::NONE,
    ));
    painter.add(egui::Shape::closed_line(points, Stroke::new(1.5, color)));
}

/// Three jobs joined into a pipeline, the last one checked: your CI, proved.
fn pipeline_mark(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let y = rect.center().y;
    let xs = [rect.left() + 2.5, rect.center().x, rect.right() - 2.5];
    painter.line_segment([Pos2::new(xs[0], y), Pos2::new(xs[2], y)], stroke);
    for x in &xs[..2] {
        painter.circle(Pos2::new(*x, y), 3.0, theme::PANEL_BG_ALT(), stroke);
    }
    let end = Pos2::new(xs[2], y);
    painter.circle_filled(end, 4.0, color);
    painter.line_segment(
        [end + Vec2::new(-2.0, 0.0), end + Vec2::new(-0.5, 1.5)],
        Stroke::new(1.2, theme::BG()),
    );
    painter.line_segment(
        [end + Vec2::new(-0.5, 1.5), end + Vec2::new(2.2, -1.6)],
        Stroke::new(1.2, theme::BG()),
    );
}

#[cfg(test)]
mod tests {
    use super::{Production, card_id, unconfigured};
    use crate::test_egui::DiscardTextures as _;
    use egui::{Rect, Shape};

    /// Draws the cards in a column `width` wide; returns each text's rect and the cards'.
    fn draw(width: f32) -> (Vec<(String, Rect)>, [Rect; 2]) {
        draw_in(&egui::Context::default(), width)
    }

    fn draw_in(ctx: &egui::Context, width: f32) -> (Vec<(String, Rect)>, [Rect; 2]) {
        let mut form = Production::default();
        // A long custom preset name must not push the button or its chip out of the card.
        let long = "Claude Code with a very long custom preset name for this team";
        form.setup_presets = vec![horizon_core::PresetConfig {
            name: long.into(),
            alias: None,
            kind: horizon_core::PanelKind::Claude,
            command: None,
            args: Vec::new(),
            resume: horizon_core::PanelResume::default(),
            ssh_connection: None,
        }];
        form.setup_preset = Some(long.into());
        form.repository = "/synthetic/repository".into();
        let mut cards = [Rect::NOTHING; 2];
        let mut texts = Vec::new();
        // Later passes draw with the heights the earlier ones measured.
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width + 40.0, 1200.0))),
                ..Default::default()
            };
            let output = ctx
                .run_ui(input, |ui| {
                    ui.set_max_width(width);
                    let _ = unconfigured(ui, &mut form);
                })
                .discard_textures();
            texts = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    Shape::Text(text) => Some((
                        text.galley.job.text.clone(),
                        Rect::from_min_size(text.pos, text.galley.size()),
                    )),
                    _ => None,
                })
                .collect();
            for (index, key) in ["quick-start", "setup-from-ci"].into_iter().enumerate() {
                cards[index] = ctx.data(|data| data.get_temp::<Rect>(card_id(key))).unwrap();
            }
        }
        (texts, cards)
    }

    #[test]
    fn every_line_stays_inside_its_card_at_any_width() {
        for width in [280.0, 540.0, 660.0, 900.0] {
            let (texts, cards) = draw(width);
            for (text, rect) in &texts {
                let Some(card) = cards
                    .iter()
                    .find(|card| card.contains(rect.left_top() + egui::vec2(1.0, 1.0)))
                else {
                    continue;
                };
                assert!(
                    card.expand(0.5).contains_rect(*rect),
                    "{width}: {text:?} at {rect:?} leaves its card {card:?}"
                );
            }
            let side_by_side = (cards[0].top() - cards[1].top()).abs() < 0.5;
            assert_eq!(side_by_side, width >= super::SIDE_BY_SIDE, "{width}");
            if side_by_side {
                assert!(
                    (cards[0].bottom() - cards[1].bottom()).abs() < 0.5,
                    "{width}: the pair ends on one line"
                );
            }
        }
    }

    #[test]
    fn widening_the_dialog_again_gives_the_cards_their_own_height_back() {
        let ctx = egui::Context::default();
        let (_, wide) = draw_in(&ctx, 900.0);
        let _ = draw_in(&ctx, 660.0);
        let _ = draw_in(&ctx, 300.0);
        let (_, again) = draw_in(&ctx, 900.0);
        assert!(
            (again[0].height() - wide[0].height()).abs() < 0.5,
            "{again:?} is not as tall as {wide:?} again"
        );
        assert!((again[0].bottom() - again[1].bottom()).abs() < 0.5);
    }
}

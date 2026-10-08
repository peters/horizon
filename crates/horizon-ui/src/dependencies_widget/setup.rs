//! The first-run steps: connect GitHub, choose repositories, reach a worker. Each step
//! says what it is for and what it waits for; nothing after GitHub unlocks early.

use egui::{Align, Align2, FontId, Layout, RichText, Sense, Stroke, StrokeKind, Vec2, pos2};
use horizon_core::maintenance::{
    GitHubApp,
    portfolio::Tone,
    setup::{GitHubStep, RepositoriesStep, Setup, WorkerStep},
};

use super::{tone, widgets};
use crate::theme;

/// The guide in the repository, for "How it works".
const GUIDE_URL: &str = "https://github.com/peters/horizon/blob/main/docs/dependencies.md";
const COLUMN: f32 = 760.0;

pub(super) enum Request {
    OpenCloudSettings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Done,
    Current,
    Waiting,
}

/// The button a step offers, if any.
struct Offer<'a> {
    label: &'a str,
    primary: bool,
    enabled: bool,
    hint: &'a str,
}

struct Step<'a> {
    number: usize,
    title: &'a str,
    body: &'a str,
    look: Look,
    status: String,
    tone: Tone,
    offer: Option<Offer<'a>>,
}

pub(super) fn show(ui: &mut egui::Ui, setup: &Setup, github: Option<&GitHubApp>) -> Option<Request> {
    let mut request = None;
    egui::ScrollArea::vertical()
        .id_salt("dependencies-setup")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width().min(COLUMN);
            let margin = ((ui.available_width() - width) / 2.0).max(0.0);
            ui.horizontal(|ui| {
                ui.add_space(margin);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    ui.add_space(12.0);
                    heading(ui, setup);
                    ui.add_space(20.0);
                    ui.spacing_mut().item_spacing.y = 12.0;
                    if card(ui, &github_step(setup, github)) {
                        request = Some(Request::OpenCloudSettings);
                    }
                    if card(ui, &repositories_step(setup, github))
                        && let Some(app) = github
                    {
                        open(&app.installation_url);
                    }
                    card(ui, &worker_step(setup));
                    ui.add_space(8.0);
                    if ui
                        .add(
                            egui::Button::new(RichText::new("How it works ↗").size(13.5).color(theme::ACCENT()))
                                .frame(false),
                        )
                        .on_hover_text(GUIDE_URL)
                        .clicked()
                    {
                        open(GUIDE_URL);
                    }
                });
            });
        });
    request
}

fn heading(ui: &mut egui::Ui, setup: &Setup) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        let (badge, _) = ui.allocate_exact_size(Vec2::splat(52.0), Sense::hover());
        widgets::dependency_badge(ui.painter(), badge);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(
                RichText::new("Set up Dependencies")
                    .size(24.0)
                    .strong()
                    .color(theme::FG()),
            );
            ui.label(
                RichText::new(format!("Step {} of 3", setup.current()))
                    .size(13.0)
                    .color(theme::FG_SOFT()),
            );
        });
    });
    ui.add_space(12.0);
    ui.label(
        RichText::new(
            "A cloud worker keeps Dependabot pull requests moving across your repositories. It follows \
             each repository's Dependabot settings and AGENTS.md, runs the checks they require, and \
             reports here.",
        )
        .size(14.0)
        .color(theme::FG_SOFT()),
    );
}

fn github_step<'a>(setup: &Setup, github: Option<&GitHubApp>) -> Step<'a> {
    let (look, status, tone, offer) = match (setup.github, github) {
        (GitHubStep::Connected, app) => (
            Look::Done,
            app.map_or_else(
                || "Connected".to_owned(),
                |app| format!("Connected · GitHub App {}", app.slug),
            ),
            Tone::Good,
            Some(Offer {
                label: "Cloud settings",
                primary: false,
                enabled: true,
                hint: "Manage the GitHub App and how clouds sign in",
            }),
        ),
        (GitHubStep::Simulated, _) => (
            Look::Done,
            "Simulated by the test worker · no GitHub account is used".to_owned(),
            Tone::Neutral,
            None,
        ),
        (GitHubStep::Needed, _) => (
            Look::Current,
            "Not connected".to_owned(),
            Tone::Warning,
            Some(Offer {
                label: "Connect GitHub…",
                primary: true,
                enabled: true,
                hint: "Opens Cloud settings at Connect GitHub",
            }),
        ),
    };
    Step {
        number: 1,
        title: "Connect GitHub",
        body: "Horizon creates its own GitHub App on your account. Each worker signs in through it for its \
               own cloud, and only reaches the repositories you choose.",
        look,
        status,
        tone,
        offer,
    }
}

fn repositories_step<'a>(setup: &Setup, github: Option<&GitHubApp>) -> Step<'a> {
    let choose = Offer {
        label: "Choose on GitHub ↗",
        primary: false,
        enabled: github.is_some(),
        hint: "Opens the app's repository access on GitHub",
    };
    let (look, status, tone, offer) = match setup.repositories {
        RepositoriesStep::Blocked => (
            Look::Waiting,
            "Connect GitHub first".to_owned(),
            Tone::Quiet,
            Some(choose),
        ),
        RepositoriesStep::Choose => (
            Look::Current,
            "Choose on GitHub · the worker confirms them when it reports".to_owned(),
            Tone::Neutral,
            github.is_some().then_some(choose),
        ),
        RepositoriesStep::Reported { count, simulated } => (
            Look::Done,
            format!(
                "{count} repositories reported by the {}",
                if simulated { "test worker" } else { "worker" }
            ),
            Tone::Good,
            github.is_some().then_some(Offer {
                label: "Change on GitHub ↗",
                ..choose
            }),
        ),
    };
    Step {
        number: 2,
        title: "Choose repositories",
        body: "Pick the repositories the app may reach. The worker maintains the ones that have a \
               .github/dependabot.yml and keeps their groups, schedules and ignore rules.",
        look,
        status,
        tone,
        offer,
    }
}

fn worker_step<'a>(setup: &Setup) -> Step<'a> {
    let start = Offer {
        label: "Start worker",
        primary: true,
        enabled: false,
        hint: "Starting a dependency worker from Horizon is not available yet",
    };
    let (look, status, tone, offer) = match &setup.worker {
        WorkerStep::Blocked => (
            Look::Waiting,
            "Connect GitHub first".to_owned(),
            Tone::Quiet,
            Some(start),
        ),
        WorkerStep::Unavailable => (
            Look::Current,
            "Not available yet: this version of Horizon cannot start a dependency worker".to_owned(),
            Tone::Quiet,
            Some(start),
        ),
        WorkerStep::Connecting => (
            Look::Current,
            "Connecting to the worker…".to_owned(),
            Tone::Active,
            None,
        ),
        WorkerStep::Unreachable(error) => (Look::Current, error.clone(), Tone::Warning, None),
        WorkerStep::Running { simulated } => (
            Look::Done,
            if *simulated {
                "Test worker running"
            } else {
                "Worker running"
            }
            .to_owned(),
            Tone::Good,
            None,
        ),
    };
    Step {
        number: 3,
        title: "Start a dependency worker",
        body: "The worker runs in the cloud, works through each repository's Dependabot pull requests and \
               keeps running when you close this panel.",
        look,
        status,
        tone,
        offer,
    }
}

/// One step as a card. Returns true when its button was clicked.
fn card(ui: &mut egui::Ui, step: &Step<'_>) -> bool {
    let current = step.look == Look::Current;
    let (fill, stroke) = if current {
        (
            theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.08),
            Stroke::new(1.0, theme::blend(theme::BORDER_SUBTLE(), theme::ACCENT(), 0.5)),
        )
    } else {
        (theme::PANEL_BG_ALT(), Stroke::new(1.0, theme::BORDER_SUBTLE()))
    };
    let mut clicked = false;
    egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(12)
        .inner_margin(18)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                number(ui, step.number, step.look);
                let offer_width = if step.offer.is_some() { 170.0 } else { 0.0 };
                ui.allocate_ui_with_layout(
                    egui::vec2((ui.available_width() - offer_width).max(160.0), 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 6.0;
                        let title = if step.look == Look::Waiting {
                            theme::FG_SOFT()
                        } else {
                            theme::FG()
                        };
                        ui.label(RichText::new(step.title).size(16.0).strong().color(title));
                        ui.label(RichText::new(step.body).size(13.5).color(theme::FG_SOFT()));
                        status(ui, &step.status, step.tone);
                    },
                );
                if let Some(offer) = &step.offer {
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        clicked = offer_button(ui, offer);
                    });
                }
            });
        });
    clicked
}

fn offer_button(ui: &mut egui::Ui, offer: &Offer<'_>) -> bool {
    let button = if offer.primary {
        widgets::primary_button(offer.label).min_size(egui::vec2(150.0, 38.0))
    } else {
        widgets::chrome_button(offer.label).min_size(egui::vec2(150.0, 38.0))
    };
    ui.add_enabled(offer.enabled, button)
        .on_hover_text(offer.hint)
        .on_disabled_hover_text(offer.hint)
        .clicked()
}

fn number(ui: &mut egui::Ui, number: usize, look: Look) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    match look {
        Look::Done => {
            painter.circle_filled(center, 15.0, theme::alpha(theme::PALETTE_GREEN(), 40));
            painter.circle_stroke(
                center,
                14.5,
                Stroke::new(1.0, theme::alpha(theme::PALETTE_GREEN(), 120)),
            );
            widgets::check_mark(painter, center, 15.0, theme::PALETTE_GREEN());
        }
        Look::Current => {
            painter.circle_filled(center, 15.0, theme::ACCENT());
            painter.text(
                center,
                Align2::CENTER_CENTER,
                number.to_string(),
                FontId::proportional(15.0),
                theme::BG(),
            );
        }
        Look::Waiting => {
            painter.circle_stroke(center, 14.5, Stroke::new(1.5, theme::BORDER_STRONG()));
            painter.text(
                center,
                Align2::CENTER_CENTER,
                number.to_string(),
                FontId::proportional(15.0),
                theme::FG_SOFT(),
            );
        }
    }
}

fn status(ui: &mut egui::Ui, text: &str, state: Tone) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
        let color = tone::color(state);
        if state == Tone::Active {
            ui.painter()
                .rect_stroke(dot.shrink(1.0), 5, Stroke::new(1.5, color), StrokeKind::Inside);
        } else {
            ui.painter()
                .circle_filled(pos2(dot.center().x, dot.center().y), 4.0, color);
        }
        ui.label(RichText::new(text).size(13.0).color(theme::FG()));
    });
}

fn open(url: &str) {
    if let Err(error) = horizon_core::open_url(url) {
        tracing::warn!(%error, "could not open a Dependencies link");
    }
}

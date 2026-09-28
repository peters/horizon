//! The drawer under a production cloud's header: everything the header summarizes,
//! one tab at a time, over the cloud's panels.
use super::super::{Runtime, companions};
use super::status::Status;
use super::steps::{self, StepAction};
use super::{Action, danger_button, profile_details, runtime_actions};
use crate::app::cloud_panel::runtime::{action_button, readable_runtime_style, solid_scroll_area};
use crate::theme;
use egui::{Align2, FontId, Rect, RichText, Sense, Stroke, pos2, vec2};
use horizon_core::cloud_panel::{CloudGroup, CloudLaunch};
use horizon_core::{Board, WorkspaceLayout};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app::cloud_panel) enum Tab {
    #[default]
    Overview,
    Output,
    Machine,
    Cost,
    Connections,
    Manage,
}

impl Tab {
    const ALL: [Self; 6] = [
        Self::Overview,
        Self::Output,
        Self::Machine,
        Self::Cost,
        Self::Connections,
        Self::Manage,
    ];
    /// Steps and output are already in the body of a cloud without panels.
    const BESIDE_BODY: [Self; 4] = [Self::Machine, Self::Cost, Self::Connections, Self::Manage];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Output => "Output",
            Self::Machine => "Machine",
            Self::Cost => "Cost",
            Self::Connections => "Connections",
            Self::Manage => "Manage",
        }
    }

    /// The drawer's height for this tab; content taller than this scrolls.
    pub(super) fn height(self) -> f32 {
        match self {
            Self::Overview => 360.0,
            Self::Output => 580.0,
            Self::Machine | Self::Manage => 420.0,
            Self::Cost => 340.0,
            Self::Connections => 460.0,
        }
    }

    /// Tabs the drawer offers while `body` shows the steps and output.
    pub(super) fn shown(body: bool) -> &'static [Self] {
        if body { &Self::BESIDE_BODY } else { &Self::ALL }
    }

    /// The tab to open for `wanted`, when the drawer offers it.
    pub(super) fn resolve(wanted: Self, body: bool) -> Self {
        let shown = Self::shown(body);
        if shown.contains(&wanted) { wanted } else { shown[0] }
    }
}

#[derive(Default)]
pub(super) enum LayoutChoice {
    #[default]
    Unchanged,
    Set(Option<WorkspaceLayout>),
}

#[derive(Default)]
pub(super) struct Response {
    pub action: Option<Action>,
    pub resize: Option<(u16, u16)>,
    /// Set when a layout button was chosen; `None` inside is manual placement.
    pub layout: LayoutChoice,
    pub fullscreen: bool,
}

pub(super) struct Context<'a> {
    pub group: &'a CloudGroup,
    pub launch: &'a CloudLaunch,
    pub board: &'a Board,
    pub status: &'a Status,
    pub companions: &'a mut companions::State,
    pub region_of: &'a dyn Fn(&str) -> Option<String>,
    pub body: bool,
    pub fullscreen: bool,
    pub teasers: [String; 6],
}

/// Draws the open drawer at `rect`'s top and returns what was chosen in it.
pub(super) fn show(ui: &mut egui::Ui, rect: Rect, runtime: &mut Runtime, mut context: Context<'_>) -> Response {
    let mut response = Response::default();
    let Some(wanted) = runtime.drawer else { return response };
    let tab = Tab::resolve(wanted, context.body);
    let height = tab.height().min(rect.height());
    let area = Rect::from_min_size(rect.min, vec2(rect.width(), height));
    ui.painter().rect_filled(
        area.translate(vec2(0.0, 8.0)),
        12,
        theme::alpha(egui::Color32::BLACK, 110),
    );
    let frame = egui::Frame::new()
        .fill(theme::PANEL_BG_ALT())
        .stroke(Stroke::new(1.0, theme::BORDER_STRONG()))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(16, 12));
    let inner = area.size() - frame.total_margin().sum();
    frame.show(ui, |ui| {
        ui.set_min_size(inner);
        ui.set_max_size(inner);
        if let Some(chosen) = tabs(ui, tab, &context) {
            runtime.drawer = Some(chosen);
        }
        ui.add_space(6.0);
        let room = ui.available_height();
        readable_runtime_style(ui);
        let id = context.group.issue;
        match tab {
            // The log scrolls by itself and keeps its own follow position.
            Tab::Output => super::output::show(ui, id, "drawer", runtime, room, context.status.failure.as_ref()),
            _ => {
                solid_scroll_area(ui)
                    .id_salt(("cloud-drawer", id, tab as u8))
                    .max_height(room)
                    .min_scrolled_height(room)
                    .show(ui, |ui| match tab {
                        Tab::Overview => overview(ui, id, runtime, &context, &mut response),
                        Tab::Machine => {
                            response.resize = profile_details(ui, id, context.launch, runtime, context.region_of);
                            ui.add_space(8.0);
                            super::machine::worker(ui, runtime);
                        }
                        Tab::Cost => super::cost::show(ui, runtime),
                        Tab::Connections => connections(ui, runtime, &mut context, &mut response),
                        Tab::Manage => manage(ui, id, runtime, &context, &mut response),
                        Tab::Output => {}
                    });
            }
        }
    });
    response
}

fn tabs(ui: &mut egui::Ui, active: Tab, context: &Context<'_>) -> Option<Tab> {
    let mut chosen = None;
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    let mut x = row.left();
    for &tab in Tab::shown(context.body) {
        let teaser = &context.teasers[tab as usize];
        let painter = ui.painter();
        let name = painter.layout_no_wrap(tab.label().to_owned(), FontId::proportional(14.0), theme::FG());
        let small = painter.layout_no_wrap(teaser.clone(), FontId::proportional(12.0), theme::FG_DIM());
        let width = name.size().x + if teaser.is_empty() { 0.0 } else { small.size().x + 8.0 } + 24.0;
        let rect = Rect::from_min_size(pos2(x, row.top()), vec2(width, 32.0));
        let response = ui.interact(rect, ui.id().with(("drawer-tab", tab as u8)), Sense::click());
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, tab == active, tab.label()));
        if tab == active {
            ui.painter().rect_filled(rect, 8, theme::alpha(theme::ACCENT(), 45));
        } else if response.hovered() {
            ui.painter().rect_filled(rect, 8, theme::alpha(theme::FG_DIM(), 30));
        }
        let label = ui.painter().text(
            pos2(rect.left() + 12.0, rect.center().y),
            Align2::LEFT_CENTER,
            tab.label(),
            FontId::proportional(14.0),
            if tab == active { theme::FG() } else { theme::FG_SOFT() },
        );
        if !teaser.is_empty() {
            let color = if tab == Tab::Output && context.status.failure.is_some() {
                theme::PALETTE_RED()
            } else {
                theme::FG_DIM()
            };
            ui.painter().text(
                pos2(label.right() + 8.0, rect.center().y),
                Align2::LEFT_CENTER,
                teaser,
                FontId::proportional(12.0),
                color,
            );
        }
        if response.clicked() {
            chosen = Some(tab);
        }
        x += width + 6.0;
    }
    ui.painter().line_segment(
        [
            pos2(row.left() - 16.0, row.bottom() + 4.0),
            pos2(row.right() + 16.0, row.bottom() + 4.0),
        ],
        Stroke::new(1.0, theme::BORDER_SUBTLE()),
    );
    ui.add_space(8.0);
    chosen
}

fn overview(ui: &mut egui::Ui, id: u32, runtime: &mut Runtime, context: &Context<'_>, response: &mut Response) {
    let status = context.status;
    steps::horizontal(ui, runtime, status);
    ui.add_space(10.0);
    if let Some(failure) = &status.failure {
        let copy = failure.copy_text();
        egui::Frame::new()
            .fill(theme::blend(theme::PANEL_BG(), theme::PALETTE_RED(), 0.08))
            .stroke(Stroke::new(1.0, theme::alpha(theme::PALETTE_RED(), 120)))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(format!("{} {} · {}", status.verb, status.tail, failure.summary))
                        .size(13.0)
                        .color(theme::FG_DIM()),
                );
                ui.label(
                    RichText::new(failure.headline())
                        .monospace()
                        .size(19.0)
                        .color(theme::PALETTE_RED()),
                );
                if let Some(meaning) = failure.meaning {
                    ui.label(RichText::new(meaning).size(14.0).color(theme::FG()));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if status.primary.is_some() && ui.add(action_button("Retry deploy")).clicked() {
                        response.action = Some(Action::Deploy);
                    }
                    if ui.add(action_button("Copy error")).clicked() {
                        ui.ctx().copy_text(copy.clone());
                    }
                    if ui.add(action_button("Open output")).clicked() {
                        runtime.drawer = Some(Tab::Output);
                    }
                });
            });
        return;
    }
    if status.live() {
        current(ui, runtime, status);
        return;
    }
    super::timeline::show(ui, id, runtime);
    ui.add_space(8.0);
    ui.label(RichText::new(&status.numbers).size(14.0).color(theme::FG_SOFT()));
    if !status.tail.is_empty() {
        ui.label(RichText::new(&status.tail).size(13.0).color(theme::FG_DIM()));
    }
}

/// The running step: where it runs, what it reports, and the newest output lines.
fn current(ui: &mut egui::Ui, runtime: &Runtime, status: &Status) {
    let stage = status.track.current.map(|index| status.track.stages[index]);
    ui.columns(2, |columns| {
        egui::Frame::new()
            .fill(theme::PANEL_BG())
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(&mut columns[0], |ui| {
                ui.set_width(ui.available_width());
                if let Some(stage) = stage {
                    ui.label(RichText::new(stage.label()).size(13.0).color(theme::FG_DIM()));
                }
                ui.label(RichText::new(&status.verb).size(22.0).color(theme::PALETTE_CYAN()));
                let measured = runtime.progress.measured();
                if let Some(measured) = &measured {
                    ui.label(RichText::new(measured.detail).size(14.0).color(theme::FG_SOFT()));
                    if let Some(fraction) = measured.fraction() {
                        steps::bar(ui, fraction, stage.map_or(theme::ACCENT(), super::strip::stage_color));
                    }
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&status.numbers).size(13.5).color(theme::FG()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(&status.tail).size(13.5).color(theme::FG_DIM()));
                    });
                });
            });
        let ui = &mut columns[1];
        ui.label(RichText::new("Latest output").size(12.0).color(theme::FG_DIM()));
        for line in runtime.logs.iter().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
            ui.add(
                egui::Label::new(RichText::new(&line.text).monospace().size(12.0).color(theme::FG_SOFT())).truncate(),
            );
        }
    });
}

fn connections(ui: &mut egui::Ui, runtime: &mut Runtime, context: &mut Context<'_>, response: &mut Response) {
    ui.label(super::attachment_summary(context.group, runtime, context.board))
        .on_hover_text("Local terminal processes are counted separately from deployment. A running process alone does not confirm the remote connection; check the terminal output.");
    if let Some(state) = &runtime.state {
        for session in &state.sessions {
            ui.small(format!(
                "{} · {} · {} · tmux {}",
                session.agent, session.branch, session.worktree, session.tmux
            ));
        }
    }
    ui.small("Sessions continue while disconnected.");
    ui.add_space(6.0);
    if super::desktop_button(ui, runtime) {
        response.action = Some(Action::Desktop);
    }
    if let Some(action) = super::super::local_network::show(ui, runtime) {
        response.action = Some(action);
    }
    if runtime.can_release_remote_devices()
        && ui
            .add(danger_button("Release devices and remove remote credentials"))
            .on_hover_text("Stops this cloud’s hosted browser sessions and private tunnel, then deletes its copied credentials. Reconnect transfers them again only while the local grant remains configured.")
            .clicked()
    {
        response.action = Some(Action::RevokeBrowserstack);
    }
    context.companions.render(ui, &context.launch.id);
}

fn manage(ui: &mut egui::Ui, id: u32, runtime: &mut Runtime, context: &Context<'_>, response: &mut Response) {
    ui.label(RichText::new("Workspace").size(12.0).color(theme::FG_DIM()));
    ui.horizontal_wrapped(|ui| {
        let mut selected = context.group.layout;
        if crate::app::workspace::workspace_layout_buttons(
            ui,
            &mut selected,
            theme::workspace_accent(context.group.issue.saturating_sub(101) as usize),
        ) {
            response.layout = LayoutChoice::Set(selected);
        }
        response.fullscreen = ui
            .add(action_button(if context.fullscreen {
                "Exit full screen"
            } else {
                "Full screen"
            }))
            .clicked();
    });
    ui.add_space(6.0);
    ui.label(RichText::new("Cloud").size(12.0).color(theme::FG_DIM()));
    response.action = runtime_actions(ui, id, runtime).or(response.action.take());
}

/// Carries a step action from the body into the same handling as the drawer's.
pub(super) fn step_action(action: StepAction, status: &Status, ctx: &egui::Context) -> Option<Action> {
    match action {
        StepAction::Retry => Some(Action::Deploy),
        StepAction::CopyError => {
            if let Some(failure) = &status.failure {
                ctx.copy_text(failure.copy_text());
            }
            None
        }
    }
}

/// Places the drawer under the header, inset from the frame, and no lower than the frame.
pub(super) fn placement(group: &CloudGroup) -> Rect {
    let (min, max) = group.bounds();
    let top = min[1] + group.header_height() + 6.0;
    Rect::from_min_max(
        pos2(min[0] + 12.0, top),
        pos2(max[0] - 12.0, max[1].max(top + 240.0) - 12.0),
    )
}

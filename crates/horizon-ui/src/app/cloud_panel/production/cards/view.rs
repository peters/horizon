//! Places a production cloud's header strip, empty-cloud body and drawer, and
//! carries what was chosen in them to the cloud.
use super::super::{Confirmation, HorizonApp, Production, Runtime};
use super::drawer::{self, Tab};
use super::status::{self, Occupancy, Primary, Status};
use super::strip::{self, Indicators, Sharing, Strip, StripAction};
use super::{Action, body};
use crate::app::view::canvas_scene_transform;
use crate::theme;
use egui::{Id, Order, Pos2, Rect, Vec2};
use horizon_core::Board;
use horizon_core::cloud_panel::{CloudGroup, PAD};
use std::time::SystemTime;

/// Terminal and panel counts for the status line.
pub(super) fn occupancy(group: &CloudGroup, board: &Board) -> Occupancy {
    let (terminals, running) = board
        .panels
        .iter()
        .filter(|panel| group.panels.contains(&panel.local_id) && panel.terminal().is_some())
        .fold((0, 0), |(total, running), panel| {
            (total + 1, running + usize::from(!panel.child_exited()))
        });
    Occupancy {
        panels: group.panels.len(),
        running,
        terminals,
    }
}

/// The bridge's indicator. The header is its one always-visible reader, so while
/// the bridge runs it repaints at the bridge's refresh to follow starts and relays.
pub(super) fn sharing(ctx: &egui::Context, runtime: &Runtime) -> Sharing {
    use super::super::local_network::{Phase, STATUS_REFRESH, Sharing as State};
    if matches!(runtime.sharing, State::On(_)) {
        ctx.request_repaint_after(STATUS_REFRESH);
    }
    match &runtime.sharing {
        State::Off => Sharing::Off,
        State::Paused { .. } => Sharing::Paused,
        State::Moved { .. } => Sharing::Moved,
        State::Refused(_) => Sharing::Failed,
        State::On(bridge) => match bridge.phase() {
            Phase::Starting => Sharing::Starting,
            Phase::Open(count) => Sharing::Open(count),
            Phase::Reconnecting => Sharing::Reconnecting,
            Phase::Failed => Sharing::Failed,
        },
    }
}

/// Whether the body shows the steps and output: a production cloud without panels.
pub(super) fn body_visible(group: &CloudGroup) -> bool {
    group.remote.is_some() && group.panels.is_empty() && !group.collapsed
}

/// The body under the header strip, inside the frame's padding.
fn body_rect(group: &CloudGroup) -> Rect {
    let (min, max) = group.bounds();
    Rect::from_min_max(
        Pos2::new(min[0] + PAD, min[1] + group.header_height() + PAD),
        Pos2::new(max[0] - PAD, max[1] - PAD),
    )
}

fn teasers(group: &CloudGroup, runtime: &Runtime, status: &Status, occupancy: Occupancy) -> [String; 6] {
    let profile = runtime
        .state
        .as_ref()
        .map(|state| &state.profile)
        .or_else(|| group.remote.as_ref().map(|launch| &launch.profile));
    let stages = status.track.stages.len();
    [
        status.track.current.map_or_else(
            || format!("{}/{stages}", status.track.finished),
            |index| format!("{}/{stages}", index + 1),
        ),
        format!("{} lines", runtime.logs.len() + runtime.pending_logs.len()),
        profile.map_or_else(String::new, |profile| format!("{} vCPU", profile.cpu)),
        super::cost::teaser(runtime),
        format!("{}/{}", occupancy.running, occupancy.terminals),
        String::new(),
    ]
}

/// Provider and profile, then where the cloud lives and its size.
fn subtitle(group: &CloudGroup, runtime: &Runtime) -> String {
    let provider = group.environment.provider.as_deref().unwrap_or("Local");
    let provider = horizon_core::cloud_runtime::provider::by_id(provider).map_or(provider, |described| described.label);
    let profile = group.environment.profile.as_deref().unwrap_or("Development");
    let mut parts = vec![format!("{provider} / {profile}")];
    if let Some(launch) = &group.remote {
        if let Some(place) = super::placement::short(launch, runtime.state.as_ref()) {
            parts.push(place);
        }
        let size = runtime.state.as_ref().map_or(&launch.profile, |state| &state.profile);
        parts.push(format!(
            "{} vCPU · {} GB{}",
            size.cpu,
            size.memory_gb,
            if size.gpu { " · GPU" } else { "" }
        ));
    }
    parts.join("  ·  ")
}

/// A cloud that takes panels, or already holds some, offers their arrangement; a failure
/// keeps the room for its sentence.
pub(super) fn layout_controls(
    group: &CloudGroup,
    status: &Status,
    occupancy: Occupancy,
) -> Option<strip::LayoutControls> {
    let usable = status.tone == status::Tone::Ready || (occupancy.panels > 0 && status.tone != status::Tone::Failed);
    usable.then(|| strip::LayoutControls {
        selected: group.layout,
        color: theme::workspace_accent(group.issue.saturating_sub(101) as usize),
    })
}

impl Production {
    /// The status strip inside a production cloud's header. `header` spans the whole
    /// header, including the status line and track under the title.
    pub(in crate::app::cloud_panel) fn header_strip(
        &mut self,
        ui: &mut egui::Ui,
        header: Rect,
        group: &CloudGroup,
        board: &Board,
    ) -> Strip {
        let now = SystemTime::now();
        let companions = group
            .remote
            .as_ref()
            .map_or(0, |launch| self.companions.selected(&launch.id));
        let runtime = self.runtimes.entry(group.issue).or_default();
        let occupancy = occupancy(group, board);
        let frame = ui.ctx().cumulative_frame_nr();
        let status = status::for_frame(runtime, occupancy, now, frame);
        let desktop = runtime
            .state
            .as_ref()
            .map(|state| &state.profile)
            .or_else(|| group.remote.as_ref().map(|launch| &launch.profile))
            .filter(|profile| profile.capabilities.desktop)
            .map(|_| runtime.desktop.is_some());
        let indicators = Indicators {
            running: occupancy.running,
            terminals: occupancy.terminals,
            desktop,
            sharing: sharing(ui.ctx(), runtime),
            companions,
        };
        let spend = super::cost::spend(runtime, now);
        let layout = layout_controls(group, &status, occupancy);
        let mut strip = strip::show(
            ui,
            header,
            &status,
            &indicators,
            &spend,
            runtime.drawer.is_some(),
            layout,
        );
        strip.subtitle = subtitle(group, runtime);
        status::keep(runtime, status, frame);
        strip
    }
}

impl HorizonApp {
    pub(in crate::app::cloud_panel) fn apply_strip_action(
        &mut self,
        id: u32,
        action: StripAction,
        ctx: &egui::Context,
    ) {
        // The drawer opens over the cloud's body, so a collapsed cloud expands for it.
        let opens_drawer = matches!(
            action,
            StripAction::ToggleDrawer | StripAction::Primary(Primary::Stop | Primary::Redeploy | Primary::Manage)
        );
        if opens_drawer
            && let Some(index) = self.cloud_prototype.groups.0.iter().position(|group| group.issue == id)
            && self.cloud_prototype.groups.0[index].collapsed
        {
            self.cloud_prototype.groups.0[index].set_collapsed(&mut self.board, false);
            self.save_cloud_prototype();
        }
        if let StripAction::Layout(layout) = action {
            if let Some(index) = self.cloud_prototype.groups.0.iter().position(|group| group.issue == id) {
                self.cloud_prototype.groups.0[index].set_layout(&mut self.board, layout);
                self.cloud_prototype.groups.make_room(&mut self.board, index);
                self.save_cloud_prototype();
            }
            return;
        }
        let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) else {
            return;
        };
        match action {
            StripAction::ToggleDrawer => {
                runtime.drawer = if runtime.drawer.is_some() {
                    None
                } else {
                    Some(Tab::default())
                };
            }
            StripAction::Layout(_) => {}
            StripAction::Primary(Primary::Cancel) => {
                if let Some(cancel) = &runtime.cancel {
                    cancel.cancel();
                }
            }
            StripAction::Primary(Primary::Stop) => {
                runtime.confirmation = Confirmation::Stop;
                runtime.drawer = Some(Tab::Manage);
                runtime.reveal_confirmation = true;
            }
            StripAction::Primary(Primary::Redeploy) => {
                runtime.confirmation = Confirmation::Redeploy;
                runtime.drawer = Some(Tab::Manage);
                runtime.reveal_confirmation = true;
            }
            StripAction::Primary(Primary::Manage) => runtime.drawer = Some(Tab::Manage),
            StripAction::Primary(Primary::Deploy | Primary::Reconnect | Primary::Retry) => {
                self.apply_card_action(id, Action::Deploy, ctx);
            }
            StripAction::Primary(Primary::Resume) => self.apply_card_action(id, Action::Resume, ctx),
            StripAction::Primary(Primary::ReconcileStop) => self.apply_card_action(id, Action::Stop, ctx),
            StripAction::Primary(Primary::CheckProvider) => self.apply_card_action(id, Action::Reconcile, ctx),
        }
    }

    pub(super) fn apply_card_action(&mut self, id: u32, action: Action, ctx: &egui::Context) {
        match action {
            Action::Deploy => self.start_production_deployment(id, ctx),
            Action::Desktop => self.cloud_add_panel(ctx, id, horizon_core::PanelKind::Device, None),
            Action::Remove => self.remove_deleted_cloud(id, ctx),
            Action::ShareLocalNetwork => self.share_local_network(id, true),
            Action::StopSharingLocalNetwork => self.share_local_network(id, false),
            _ => self.change_production_worker(id, action, ctx),
        }
    }

    /// Open drawers on screen: canvas gestures such as Ctrl-double-click do not reach
    /// through them, while the rest of the canvas and its panels stay interactive.
    pub(in crate::app) fn cloud_drawer_screen_rects(&self, ctx: &egui::Context) -> Vec<Rect> {
        let canvas = self.canvas_rect(ctx);
        let transform = canvas_scene_transform(canvas, self.canvas_view);
        self.cloud_prototype
            .groups
            .0
            .iter()
            .filter(|group| {
                self.cloud_prototype
                    .fullscreen
                    .as_ref()
                    .is_none_or(|view| view.id == group.issue)
            })
            .filter_map(|group| self.cloud_drawer_rect(group))
            .map(|rect| transform * rect)
            .collect()
    }

    /// The drawer's canvas rectangle when it is open, for input routing.
    pub(in crate::app) fn cloud_drawer_rect(&self, group: &CloudGroup) -> Option<Rect> {
        let runtime = self.cloud_prototype.production.runtimes.get(&group.issue)?;
        let tab = Tab::resolve(runtime.drawer?, body_visible(group));
        (!group.collapsed).then(|| {
            let rect = drawer::placement(group);
            Rect::from_min_size(rect.min, Vec2::new(rect.width(), tab.height().min(rect.height())))
        })
    }

    pub(in crate::app::cloud_panel) fn render_production_runtimes(&mut self, ctx: &egui::Context) {
        self.ensure_cloud_provider_logo(ctx);
        let canvas = self.canvas_rect(ctx);
        let layer = Layer {
            transform: canvas_scene_transform(canvas, self.canvas_view),
            clip: canvas_scene_transform(canvas, self.canvas_view).inverse() * canvas,
        };
        let mut chosen = Chosen::default();
        self.request_landed_regions(ctx);
        let now = SystemTime::now();
        let production = &mut self.cloud_prototype.production;
        let prices = &production.prices;
        let region_of = |center: &str| prices.region_of(center).map(str::to_owned);
        let fullscreen_active = self.cloud_prototype.fullscreen.is_some();
        for group in &self.cloud_prototype.groups.0 {
            let Some(launch) = &group.remote else { continue };
            let runtime = production.runtimes.entry(group.issue).or_default();
            // Before any skip: a view hidden by another cloud's full screen reports no
            // scrolled-up reader, so its output keeps joining instead of piling up held.
            super::output::begin_frame(runtime);
            if self
                .cloud_prototype
                .fullscreen
                .as_ref()
                .is_some_and(|f| f.id != group.issue)
            {
                continue;
            }
            if group.collapsed {
                runtime.drawer = None;
            }
            let body = body_visible(group);
            // Nothing is scanned or laid out for a cloud whose body and drawer are both
            // hidden or off screen; its header already has its status.
            let on_screen = |rect: Rect| rect.intersects(layer.clip);
            let body_shown = body && on_screen(body_rect(group));
            let drawer_shown = runtime.drawer.is_some() && on_screen(drawer::placement(group));
            if !body_shown && !drawer_shown {
                continue;
            }
            let occupancy = occupancy(group, &self.board);
            let frame = ctx.cumulative_frame_nr();
            let status = status::for_frame(runtime, occupancy, now, frame);
            if body_shown && let Some(action) = body_area(ctx, &layer, group, runtime, &status) {
                chosen.actions.push((group.issue, action));
            }
            if drawer_shown {
                let teasers = teasers(group, runtime, &status, occupancy);
                let context = drawer::Context {
                    group,
                    launch,
                    board: &self.board,
                    status: &status,
                    companions: &mut production.companions,
                    region_of: &region_of,
                    body,
                    fullscreen: fullscreen_active,
                    teasers,
                };
                let response = drawer_area(ctx, &layer, group, runtime, context);
                chosen.record(group.issue, &response);
            }
            status::keep(runtime, status, frame);
        }
        self.apply_chosen(chosen, ctx);
    }

    fn apply_chosen(&mut self, chosen: Chosen, ctx: &egui::Context) {
        for (id, action) in chosen.actions {
            self.apply_card_action(id, action, ctx);
        }
        if let Some(id) = chosen.fullscreen {
            self.toggle_cloud_fullscreen(ctx, id);
        }
        if let Some((id, size)) = chosen.resize {
            self.resize_production_cloud(id, size);
        }
        if let Some((id, layout)) = chosen.layout
            && let Some(index) = self.cloud_prototype.groups.0.iter().position(|g| g.issue == id)
        {
            self.cloud_prototype.groups.0[index].set_layout(&mut self.board, layout);
            self.cloud_prototype.groups.make_room(&mut self.board, index);
            self.save_cloud_prototype();
        }
    }
}

/// The canvas transform and visible canvas region every card layer shares.
struct Layer {
    transform: egui::emath::TSTransform,
    clip: Rect,
}

/// What the cards asked for this frame, applied after they are drawn.
#[derive(Default)]
struct Chosen {
    actions: Vec<(u32, Action)>,
    fullscreen: Option<u32>,
    layout: Option<(u32, Option<horizon_core::WorkspaceLayout>)>,
    resize: Option<(u32, (u16, u16))>,
}

impl Chosen {
    fn record(&mut self, id: u32, response: &drawer::Response) {
        if let Some(action) = response.action {
            self.actions.push((id, action));
        }
        if let Some(size) = response.resize {
            self.resize = Some((id, size));
        }
        if let drawer::LayoutChoice::Set(selected) = &response.layout {
            let selected = *selected;
            self.layout = Some((id, selected));
        }
        if response.fullscreen {
            self.fullscreen = Some(id);
        }
    }
}

/// The steps and output of a cloud without panels; the same layer id as the old
/// runtime card, so input routing keeps treating it as the cloud's runtime.
fn body_area(
    ctx: &egui::Context,
    layer: &Layer,
    group: &CloudGroup,
    runtime: &mut Runtime,
    status: &Status,
) -> Option<Action> {
    let launch = group.remote.as_ref()?;
    let rect = body_rect(group);
    let step = egui::Area::new(Id::new(("cloud-runtime", group.issue)))
        .order(Order::Middle)
        .fixed_pos(rect.min)
        .constrain(false)
        .show(ctx, |ui| {
            ctx.set_transform_layer(ui.layer_id(), layer.transform);
            ui.set_clip_rect(layer.clip);
            super::super::super::runtime::readable_runtime_style(ui);
            body::show(ui, group.issue, rect.size(), launch, runtime, status)
        })
        .inner;
    step.and_then(|step| drawer::step_action(step, status, ctx))
}

/// The drawer over the cloud's panels, above them in the Foreground order.
fn drawer_area(
    ctx: &egui::Context,
    layer: &Layer,
    group: &CloudGroup,
    runtime: &mut Runtime,
    context: drawer::Context<'_>,
) -> drawer::Response {
    let rect = drawer::placement(group);
    egui::Area::new(Id::new(("cloud-drawer", group.issue)))
        .order(Order::Foreground)
        .fixed_pos(rect.min)
        .constrain(false)
        .show(ctx, |ui| {
            ctx.set_transform_layer(ui.layer_id(), layer.transform);
            ui.set_clip_rect(layer.clip);
            drawer::show(ui, rect, runtime, context)
        })
        .inner
}

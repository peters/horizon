mod controls;
mod layout;
mod paint;
mod query;

use std::time::{Duration, Instant};

use egui::{
    Align, Color32, Context, CornerRadius, EventFilter, FontId, Id, Layout, Margin, Order, Rect, RichText, ScrollArea,
    Sense, Stroke, StrokeKind, UiBuilder, Vec2,
};
use horizon_core::{RemoteHost, RemoteHostCatalog, RemoteHostConnectionSummary, SshConnection, WorkspaceId};

use self::controls::{
    DestinationEntry, DestinationPickerAction, RowMenuChoice, cycle_destination, destination_entries,
    normalize_destination, render_destination_picker, render_mode_toggle,
};
use self::layout::{
    Columns, DESTINATION_ROW_HEIGHT, INPUT_HEIGHT, OverlayLayout, columns, current_epoch_secs, overlay_layout,
};
use self::paint::{HostRowRenderContext, paint_empty, render_column_headers, render_host_details, render_host_row};
use self::query::{connect_action, filtered_indices, parse_user_prefix};
use crate::command_palette::render::paint_card;
use crate::theme;

const NOTICE_DURATION: Duration = Duration::from_secs(4);

pub struct RemoteHostsOverlay {
    query: String,
    selected: usize,
    expanded_host: Option<ExpandedHostId>,
    opened_at: Instant,
    mode: RemoteConnectMode,
    destination: WorkspaceChoice,
    /// Short-lived feedback shown in place of the host count.
    notice: Option<(String, Instant)>,
}

/// What opening a host creates: a terminal over SSH, or a read-only Device
/// panel that reaches the host's loopback VNC server through an SSH tunnel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RemoteConnectMode {
    #[default]
    Ssh,
    Vnc,
}

impl RemoteConnectMode {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ssh => "SSH",
            Self::Vnc => "VNC",
        }
    }

    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::Ssh => Self::Vnc,
            Self::Vnc => Self::Ssh,
        }
    }

    const fn hint(self) -> &'static str {
        match self {
            Self::Ssh => "Open a terminal on the host over SSH (Tab switches)",
            Self::Vnc => "View the host's desktop over VNC through an SSH tunnel (Tab switches)",
        }
    }
}

/// Which workspace receives the new panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum WorkspaceChoice {
    /// The configured default workspace, created when missing.
    #[default]
    Default,
    Existing(WorkspaceId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceOption {
    pub id: WorkspaceId,
    pub name: String,
}

pub struct RemoteHostsOverlayInputs<'a> {
    pub catalog: &'a RemoteHostCatalog,
    pub connection_summaries: &'a [RemoteHostConnectionSummary],
    pub refresh_in_flight: bool,
    /// Seconds until next auto-refresh, or `None` if refreshing or no timer.
    pub next_refresh_secs: Option<u64>,
    pub workspaces: &'a [WorkspaceOption],
    pub default_workspace: &'a str,
}

#[derive(Debug)]
pub enum RemoteHostsOverlayAction {
    None,
    Cancelled,
    Open {
        label: String,
        connection: SshConnection,
        mode: RemoteConnectMode,
        destination: WorkspaceChoice,
    },
    /// The picked workspace should become the configured default, by name.
    SetDefaultWorkspace(String),
    /// Store the host as a preset so any workspace can add it later.
    SaveShortcut {
        label: String,
        connection: SshConnection,
        mode: RemoteConnectMode,
    },
}

/// What a click or menu pick on a host row asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowRequest {
    Connect(usize),
    Menu(usize, RowMenuChoice),
}

/// Alt+D is a command here, but the platform also reports it as typed text
/// (`d`, or `∂` on a macOS US layout), which the filter box would insert.
/// Drop the text event that follows the key event, whatever it says.
fn swallow_alt_shortcut_text(ctx: &Context) {
    ctx.input_mut(|input| {
        let mut correlated_text = false;
        input.events.retain(|event| {
            let consume = correlated_text && matches!(event, egui::Event::Text(_));
            correlated_text = matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::D,
                    pressed: true,
                    modifiers,
                    ..
                } if *modifiers == egui::Modifiers::ALT
            );
            !consume
        });
    });
}

#[derive(Default)]
struct KeyPresses {
    up: bool,
    down: bool,
    enter: bool,
    escape: bool,
    tab: bool,
    alt_up: bool,
    alt_down: bool,
    alt_default: bool,
}

impl KeyPresses {
    /// A held Tab must not flip the mode on every auto-repeat; the arrows keep
    /// repeating so a held key keeps moving.
    fn record(&mut self, key: egui::Key, alt: bool, repeat: bool) {
        if repeat && key == egui::Key::Tab {
            return;
        }
        match (key, alt) {
            (egui::Key::ArrowUp, false) => self.up = true,
            (egui::Key::ArrowDown, false) => self.down = true,
            (egui::Key::ArrowUp, true) => self.alt_up = true,
            (egui::Key::ArrowDown, true) => self.alt_down = true,
            (egui::Key::D, true) => self.alt_default = true,
            (egui::Key::Enter, _) => self.enter = true,
            (egui::Key::Escape, _) => self.escape = true,
            (egui::Key::Tab, _) => self.tab = true,
            _ => {}
        }
    }
}

struct FrameContext<'a> {
    refresh_in_flight: bool,
    user_override: Option<&'a str>,
    now_secs: i64,
    /// Seconds until next auto-refresh, or `None` if refreshing or no timer.
    next_refresh_secs: Option<u64>,
    destinations: &'a [DestinationEntry],
    /// Destination controls on their own row under the filter.
    compact_header: bool,
}

struct OverlayRenderContext<'a, 'b> {
    catalog: &'a RemoteHostCatalog,
    connection_summaries: &'a [RemoteHostConnectionSummary],
    filtered: &'a [usize],
    layout: &'a OverlayLayout,
    frame: &'b FrameContext<'a>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExpandedHostId {
    label: String,
    connection: SshConnection,
}

impl RemoteHostsOverlay {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            selected: 0,
            expanded_host: None,
            opened_at: Instant::now(),
            mode: RemoteConnectMode::default(),
            destination: WorkspaceChoice::default(),
            notice: None,
        }
    }

    /// Show `text` in the header for a few seconds, e.g. after a saved shortcut or default.
    pub fn set_notice(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), Instant::now()));
    }

    fn current_notice(&mut self) -> Option<&str> {
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, shown_at)| shown_at.elapsed() > NOTICE_DURATION)
        {
            self.notice = None;
        }
        self.notice.as_ref().map(|(text, _)| text.as_str())
    }

    pub fn show(&mut self, ctx: &Context, inputs: &RemoteHostsOverlayInputs<'_>) -> RemoteHostsOverlayAction {
        swallow_alt_shortcut_text(ctx);
        let (user_override, filter_query) = parse_user_prefix(&self.query);
        let user_override = user_override.map(str::to_string);
        let filtered = filtered_indices(&inputs.catalog.hosts, filter_query);
        let layout = overlay_layout(ctx.input(egui::InputState::viewport_rect));
        let destinations = destination_entries(inputs.workspaces, inputs.default_workspace);
        normalize_destination(&mut self.destination, &destinations);
        let frame = FrameContext {
            refresh_in_flight: inputs.refresh_in_flight,
            user_override: user_override.as_deref(),
            now_secs: current_epoch_secs(),
            next_refresh_secs: inputs.next_refresh_secs,
            destinations: &destinations,
            compact_header: layout.compact_header,
        };
        let render = OverlayRenderContext {
            catalog: inputs.catalog,
            connection_summaries: inputs.connection_summaries,
            filtered: &filtered,
            layout: &layout,
            frame: &frame,
        };

        // Keep the countdown ticking while overlay is visible.
        ctx.request_repaint_after(Duration::from_secs(1));

        if self.show_backdrop(ctx, layout.screen) {
            return RemoteHostsOverlayAction::Cancelled;
        }

        self.clamp_selection(filtered.len());
        self.show_modal(ctx, &render)
    }

    fn clamp_selection(&mut self, count: usize) {
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    fn show_backdrop(&self, ctx: &Context, screen_rect: Rect) -> bool {
        let mut cancelled = false;
        // The sidebar lives on the Tooltip layer; a newly shown backdrop on the
        // same layer lands above it, so the whole window dims.
        egui::Area::new(Id::new("remote_hosts_backdrop"))
            .fixed_pos(screen_rect.min)
            .constrain(false)
            .order(Order::Tooltip)
            .interactable(true)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(screen_rect.size(), Sense::click());
                ui.painter_at(rect)
                    .rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(140));
                if response.clicked() && self.opened_at.elapsed().as_millis() > 200 {
                    cancelled = true;
                }
            });
        cancelled
    }

    fn show_modal(&mut self, ctx: &Context, render: &OverlayRenderContext<'_, '_>) -> RemoteHostsOverlayAction {
        let mut action = RemoteHostsOverlayAction::None;

        // Tooltip rather than Debug: the card must clear the sidebar, and its
        // own popups open on the Tooltip layer so they can draw above it.
        // Clicking the backdrop raises it within the Tooltip order and egui
        // keeps that order across overlay instances, so the card is registered
        // as the backdrop's sublayer every pass: sublayers are spliced directly
        // above their parent, whatever was raised last.
        let backdrop_layer = egui::LayerId::new(Order::Tooltip, Id::new("remote_hosts_backdrop"));
        let modal_layer = egui::LayerId::new(Order::Tooltip, Id::new("remote_hosts_modal"));
        ctx.memory_mut(|memory| memory.areas_mut().set_sublayer(backdrop_layer, modal_layer));
        egui::Area::new(Id::new("remote_hosts_modal"))
            .fixed_pos(render.layout.card.min)
            .constrain(true)
            .order(Order::Tooltip)
            .show(ctx, |ui| {
                paint_card(ui, render.layout.card);

                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(render.layout.inner)
                        .layout(Layout::top_down(Align::Min)),
                    |ui| {
                        action = self.show_contents(ui, ctx, render);
                    },
                );
            });

        action
    }

    fn show_contents(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &Context,
        render: &OverlayRenderContext<'_, '_>,
    ) -> RemoteHostsOverlayAction {
        let total = render.catalog.hosts.len();
        if let Some(action) =
            self.render_query_input(ui, render.layout.inner, render.filtered.len(), total, render.frame)
        {
            return action;
        }
        if let Some(action) = self.handle_keyboard(ctx, render.catalog, render.filtered, render.frame) {
            return action;
        }

        ui.allocate_space(Vec2::new(render.layout.inner.width(), INPUT_HEIGHT));
        if render.frame.compact_header {
            let row = ui
                .allocate_space(Vec2::new(render.layout.inner.width(), DESTINATION_ROW_HEIGHT))
                .1;
            let mut child = ui.new_child(
                UiBuilder::new()
                    .max_rect(row.shrink2(Vec2::new(14.0, 3.0)))
                    .layout(Layout::right_to_left(Align::Center)),
            );
            if let Some(action) = self.render_destination_controls(&mut child, render.frame.destinations) {
                return action;
            }
        }
        ui.add_space(6.0);

        let columns = columns(render.layout.inner.width());
        render_column_headers(ui, render.layout.inner.width(), &columns);

        let separator_rect = ui.allocate_space(Vec2::new(render.layout.inner.width(), 1.0)).1;
        ui.painter_at(separator_rect).rect_filled(
            separator_rect,
            CornerRadius::ZERO,
            theme::alpha(theme::BORDER_SUBTLE(), 100),
        );
        ui.add_space(2.0);

        match self.render_results(ui, &columns, render) {
            Some(RowRequest::Connect(index)) => {
                let host = &render.catalog.hosts[render.filtered[index]];
                self.open_action(host, render.frame.user_override)
            }
            Some(RowRequest::Menu(index, choice)) => {
                let host = &render.catalog.hosts[render.filtered[index]];
                self.menu_action(host, render.frame.user_override, choice)
            }
            None => RemoteHostsOverlayAction::None,
        }
    }

    fn open_action(&self, host: &RemoteHost, user_override: Option<&str>) -> RemoteHostsOverlayAction {
        connect_action(host, user_override, self.mode, self.destination.clone())
    }

    fn menu_action(
        &self,
        host: &RemoteHost,
        user_override: Option<&str>,
        choice: RowMenuChoice,
    ) -> RemoteHostsOverlayAction {
        match choice {
            RowMenuChoice::Open(mode) => connect_action(host, user_override, mode, self.destination.clone()),
            RowMenuChoice::SaveShortcut(mode) => {
                let mut connection = host.ssh_connection.clone();
                if let Some(user) = user_override {
                    connection.user = Some(user.to_string());
                }
                RemoteHostsOverlayAction::SaveShortcut {
                    label: host.label.clone(),
                    connection,
                    mode,
                }
            }
        }
    }

    fn render_query_input(
        &mut self,
        ui: &mut egui::Ui,
        inner_rect: Rect,
        filtered_count: usize,
        total_count: usize,
        frame: &FrameContext<'_>,
    ) -> Option<RemoteHostsOverlayAction> {
        let input_rect = Rect::from_min_size(ui.cursor().min, Vec2::new(inner_rect.width(), INPUT_HEIGHT));

        ui.painter()
            .rect_filled(input_rect, CornerRadius::same(12), theme::BG_ELEVATED());
        ui.painter().rect_stroke(
            input_rect,
            CornerRadius::same(12),
            Stroke::new(1.0_f32, theme::alpha(theme::ACCENT(), 70)),
            StrokeKind::Inside,
        );

        let text_rect = input_rect.shrink2(Vec2::new(14.0, 6.0));
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(text_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );

        child.label(
            RichText::new("Remote")
                .font(FontId::proportional(13.0))
                .color(theme::FG_SOFT())
                .strong(),
        );
        child.add_space(6.0);
        render_mode_toggle(&mut child, &mut self.mode);
        child.add_space(4.0);
        child.label(
            RichText::new(" > ")
                .font(FontId::monospace(13.0))
                .color(theme::ACCENT()),
        );

        let reserved_right = if frame.compact_header { 330.0 } else { 640.0 };
        let response = child.add(
            egui::TextEdit::singleline(&mut self.query)
                .font(FontId::monospace(14.0))
                .text_color(theme::FG())
                .frame(egui::Frame::NONE)
                .desired_width((text_rect.width() - reserved_right).max(160.0))
                .hint_text(
                    RichText::new("type to filter, prefix user@ to connect as that user")
                        .color(theme::FG_DIM())
                        .font(FontId::monospace(11.0)),
                )
                .margin(Margin::ZERO),
        );
        if !response.has_focus() && self.opened_at.elapsed().as_millis() < 100 {
            response.request_focus();
        }
        // Tab switches SSH/VNC instead of moving focus out of the filter.
        child.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                response.id,
                EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..EventFilter::default()
                },
            );
        });
        if response.changed() {
            self.selected = 0;
        }

        let mut action = None;
        child.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let refresh_status = if frame.refresh_in_flight {
                "refreshing...".to_string()
            } else if let Some(secs) = frame.next_refresh_secs {
                format!("{secs}s")
            } else {
                String::new()
            };
            let count_text = if refresh_status.is_empty() {
                format!("{filtered_count}/{total_count}")
            } else {
                format!("{filtered_count}/{total_count}  {refresh_status}")
            };
            match self.current_notice() {
                Some(notice) => {
                    ui.label(
                        RichText::new(notice)
                            .font(FontId::proportional(11.5))
                            .color(theme::ACCENT()),
                    );
                    ui.ctx().request_repaint_after(NOTICE_DURATION);
                }
                None => {
                    ui.label(
                        RichText::new(count_text)
                            .font(FontId::monospace(11.0))
                            .color(theme::FG_SOFT()),
                    );
                }
            }
            if !frame.compact_header {
                ui.add_space(10.0);
                action = self.render_destination_controls(ui, frame.destinations);
            }
        });
        action
    }

    fn render_destination_controls(
        &mut self,
        ui: &mut egui::Ui,
        destinations: &[DestinationEntry],
    ) -> Option<RemoteHostsOverlayAction> {
        if render_destination_picker(ui, &mut self.destination, destinations) == DestinationPickerAction::SetDefault {
            return self.set_default_action(destinations);
        }
        None
    }

    fn set_default_action(&self, destinations: &[DestinationEntry]) -> Option<RemoteHostsOverlayAction> {
        self.selected_destination_name(destinations)
            .map(RemoteHostsOverlayAction::SetDefaultWorkspace)
    }

    /// The real name of the picked workspace, never its disambiguated label;
    /// nothing when another workspace shares that name, since a default is
    /// stored by name and could not single the picked one out.
    fn selected_destination_name(&self, destinations: &[DestinationEntry]) -> Option<String> {
        destinations
            .iter()
            .find(|entry| entry.choice == self.destination && entry.choice != WorkspaceChoice::Default)
            .filter(|entry| !entry.ambiguous)
            .map(|entry| entry.name.clone())
    }

    fn handle_keyboard(
        &mut self,
        ctx: &Context,
        catalog: &RemoteHostCatalog,
        filtered: &[usize],
        frame: &FrameContext<'_>,
    ) -> Option<RemoteHostsOverlayAction> {
        // Alt is the destination modifier: Tab stays on the mode toggle and
        // the plain arrows keep moving through the host rows. Each press
        // carries its own modifiers, so a held Alt never leaks into the rows.
        let mut keys = KeyPresses::default();
        ctx.input(|input| {
            for event in &input.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } = event
                {
                    keys.record(*key, *modifiers == egui::Modifiers::ALT, *repeat);
                }
            }
        });
        let KeyPresses {
            up,
            down,
            enter,
            escape,
            tab,
            alt_up,
            alt_down,
            alt_default,
        } = keys;

        if escape {
            return Some(RemoteHostsOverlayAction::Cancelled);
        }
        if tab {
            self.mode = self.mode.toggled();
        }
        if alt_down || alt_up {
            cycle_destination(&mut self.destination, frame.destinations, alt_down);
        }
        if alt_default && let Some(action) = self.set_default_action(frame.destinations) {
            return Some(action);
        }
        if up && self.selected > 0 {
            self.selected -= 1;
        }
        if down && !filtered.is_empty() && self.selected < filtered.len() - 1 {
            self.selected += 1;
        }
        if enter && !filtered.is_empty() {
            let host = &catalog.hosts[filtered[self.selected]];
            return Some(self.open_action(host, frame.user_override));
        }

        None
    }

    fn render_results(
        &mut self,
        ui: &mut egui::Ui,
        columns: &Columns,
        render: &OverlayRenderContext<'_, '_>,
    ) -> Option<RowRequest> {
        if render.filtered.is_empty() {
            paint_empty(ui, "No matching hosts");
            return None;
        }

        let mut request = None;
        let scroll_height = render
            .layout
            .results_height
            .min(render.layout.inner.max.y - ui.cursor().min.y - 8.0);

        ScrollArea::vertical()
            .max_height(scroll_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(render.layout.inner.width());

                for (filtered_idx, host_idx) in render.filtered.iter().copied().enumerate() {
                    let host = &render.catalog.hosts[host_idx];
                    let summary = &render.connection_summaries[host_idx];
                    let is_selected = self.selected == filtered_idx;
                    let is_expanded = self.is_expanded(host);
                    let interaction = render_host_row(
                        ui,
                        &HostRowRenderContext {
                            width: render.layout.inner.width(),
                            host,
                            summary,
                            is_selected,
                            is_expanded,
                            columns,
                            now_secs: render.frame.now_secs,
                        },
                    );
                    if interaction.select {
                        self.selected = filtered_idx;
                    }
                    if interaction.toggle_expand {
                        self.toggle_expanded(host);
                    }
                    if interaction.connect {
                        request = Some(RowRequest::Connect(filtered_idx));
                    }
                    if let Some(choice) = interaction.menu {
                        request = Some(RowRequest::Menu(filtered_idx, choice));
                    }
                    if is_expanded {
                        render_host_details(
                            ui,
                            render.layout.inner.width() - 4.0,
                            host,
                            summary,
                            render.frame.now_secs,
                        );
                        ui.add_space(6.0);
                    }
                }
            });

        request
    }

    fn is_expanded(&self, host: &RemoteHost) -> bool {
        self.expanded_host
            .as_ref()
            .is_some_and(|expanded| *expanded == ExpandedHostId::from(host))
    }

    fn toggle_expanded(&mut self, host: &RemoteHost) {
        let expanded_id = ExpandedHostId::from(host);
        if self.expanded_host.as_ref() == Some(&expanded_id) {
            self.expanded_host = None;
        } else {
            self.expanded_host = Some(expanded_id);
        }
    }
}

impl From<&RemoteHost> for ExpandedHostId {
    fn from(host: &RemoteHost) -> Self {
        Self {
            label: host.label.clone(),
            connection: host.ssh_connection.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::{
        RemoteHost, RemoteHostCatalog, RemoteHostConnectionSummary, RemoteHostSources, RemoteHostStatus, SshConnection,
        WorkspaceId,
    };

    use std::time::{Duration, Instant};

    use super::controls::RowMenuChoice;
    use super::{
        KeyPresses, NOTICE_DURATION, RemoteConnectMode, RemoteHostsOverlay, RemoteHostsOverlayAction,
        RemoteHostsOverlayInputs, WorkspaceChoice, WorkspaceOption,
    };
    use crate::test_egui::DiscardTextures;

    fn show_overlay(
        ctx: &egui::Context,
        overlay: &mut RemoteHostsOverlay,
        catalog: &RemoteHostCatalog,
        workspaces: &[WorkspaceOption],
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        show_overlay_probing(ctx, overlay, catalog, workspaces, events, None).0
    }

    /// Also report which layer is on top at `probe`; that is only defined while the pass runs.
    fn show_overlay_probing(
        ctx: &egui::Context,
        overlay: &mut RemoteHostsOverlay,
        catalog: &RemoteHostCatalog,
        workspaces: &[WorkspaceOption],
        events: Vec<egui::Event>,
        probe: Option<egui::Pos2>,
    ) -> (egui::FullOutput, Option<egui::LayerId>) {
        let (output, layer, _) = show_overlay_collecting(ctx, overlay, catalog, workspaces, events, probe);
        (output, layer)
    }

    fn show_overlay_collecting(
        ctx: &egui::Context,
        overlay: &mut RemoteHostsOverlay,
        catalog: &RemoteHostCatalog,
        workspaces: &[WorkspaceOption],
        events: Vec<egui::Event>,
        probe: Option<egui::Pos2>,
    ) -> (egui::FullOutput, Option<egui::LayerId>, RemoteHostsOverlayAction) {
        let summaries = vec![RemoteHostConnectionSummary::default(); catalog.hosts.len()];
        let mut top_layer = None;
        let mut action = RemoteHostsOverlayAction::None;
        let output = ctx
            .run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
                    ..Default::default()
                },
                |ui| {
                    action = overlay.show(
                        ui.ctx(),
                        &RemoteHostsOverlayInputs {
                            catalog,
                            connection_summaries: &summaries,
                            refresh_in_flight: false,
                            next_refresh_secs: None,
                            workspaces,
                            default_workspace: "Remote Sessions",
                        },
                    );
                    top_layer = probe.and_then(|pos| ui.ctx().layer_id_at(pos));
                },
            )
            .discard_textures();
        (output, top_layer, action)
    }

    fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn alt_arrows_cycle_the_destination_and_alt_d_makes_it_the_default() {
        let ctx = egui::Context::default();
        let catalog = RemoteHostCatalog {
            hosts: vec![remote_host("live-a", 22429)],
            refreshed_at: None,
        };
        let workspaces = vec![WorkspaceOption {
            id: WorkspaceId(7),
            name: "Ops".into(),
        }];
        let mut overlay = RemoteHostsOverlay::new();
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());

        show_overlay(
            &ctx,
            &mut overlay,
            &catalog,
            &workspaces,
            vec![key_event(egui::Key::ArrowDown, egui::Modifiers::ALT)],
        );
        assert_eq!(overlay.destination, WorkspaceChoice::Existing(WorkspaceId(7)));
        assert_eq!(overlay.selected, 0, "Alt+Down leaves the host selection alone");

        // Platforms report Alt+D as a key press followed by typed text: `d` on
        // X11, `∂` on a macOS US layout. Unrelated text keeps flowing.
        overlay.query = "smoke".into();
        let (_, _, action) = show_overlay_collecting(
            &ctx,
            &mut overlay,
            &catalog,
            &workspaces,
            vec![
                key_event(egui::Key::D, egui::Modifiers::ALT),
                egui::Event::Text("\u{2202}".into()),
                egui::Event::Text("x".into()),
            ],
            None,
        );
        assert!(matches!(action, RemoteHostsOverlayAction::SetDefaultWorkspace(ref name) if name == "Ops"));
        assert!(
            !overlay.query.contains('\u{2202}'),
            "the shortcut's text never reaches the filter"
        );
        assert_eq!(overlay.query.replace('x', ""), "smoke");
        assert_eq!(overlay.query.matches('x').count(), 1, "unrelated text keeps flowing");

        show_overlay(
            &ctx,
            &mut overlay,
            &catalog,
            &workspaces,
            vec![key_event(egui::Key::ArrowUp, egui::Modifiers::ALT)],
        );
        assert_eq!(overlay.destination, WorkspaceChoice::Default);
    }

    fn text_shapes<'a>(shape: &'a egui::Shape, out: &mut Vec<&'a egui::epaint::TextShape>) {
        match shape {
            egui::Shape::Text(text) => out.push(text),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| text_shapes(shape, out)),
            _ => {}
        }
    }

    fn text_center(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        let mut texts = Vec::new();
        for shape in &output.shapes {
            text_shapes(&shape.shape, &mut texts);
        }
        texts.iter().find(|text| text.galley.text() == label).map_or_else(
            || {
                let seen: Vec<_> = texts.iter().map(|text| text.galley.text()).collect();
                panic!("missing label {label}; saw {seen:?}")
            },
            |text| text.pos + text.galley.size() * 0.5,
        )
    }

    fn click_events(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn destination_picker_opens_above_the_card_and_selects_a_workspace() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let catalog = RemoteHostCatalog {
            hosts: vec![remote_host("live-a", 22429)],
            refreshed_at: None,
        };
        let workspaces = vec![WorkspaceOption {
            id: WorkspaceId(7),
            name: "Ops".into(),
        }];
        let mut overlay = RemoteHostsOverlay::new();

        // A brand-new area is laid out invisibly on its first pass.
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        let output = show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        let picker = text_center(&output, "Remote Sessions (new)  \u{25be}");
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, click_events(picker, true));
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, click_events(picker, false));
        let output = show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        let ops = text_center(&output, "Ops");
        assert_eq!(overlay.destination, WorkspaceChoice::Default);

        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, click_events(ops, true));
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, click_events(ops, false));
        let output = show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        assert_eq!(overlay.destination, WorkspaceChoice::Existing(WorkspaceId(7)));
        text_center(&output, "Set default");
        text_center(&output, "Ops  \u{25be}");
    }

    fn button_events(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn a_right_click_menu_above_the_card_saves_a_shortcut() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let catalog = RemoteHostCatalog {
            hosts: vec![remote_host("live-a", 22429)],
            refreshed_at: None,
        };
        let workspaces = Vec::new();
        let mut overlay = RemoteHostsOverlay::new();
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        let output = show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        let row = text_center(&output, "live-a");

        // Press and release in one frame, like a slow display delivers them.
        let mut right_click = button_events(row, egui::PointerButton::Secondary, true);
        right_click.extend(button_events(row, egui::PointerButton::Secondary, false));
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, right_click);
        let output = show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        assert_eq!(overlay.selected, 0);
        let save = text_center(&output, "Save VNC shortcut");
        let (_, layer) = show_overlay_probing(&ctx, &mut overlay, &catalog, &workspaces, Vec::new(), Some(save));
        assert_ne!(
            layer.expect("a layer under the menu").id,
            egui::Id::new("remote_hosts_modal"),
            "menu hidden below the card"
        );

        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, click_events(save, true));
        let (_, _, action) = show_overlay_collecting(
            &ctx,
            &mut overlay,
            &catalog,
            &workspaces,
            click_events(save, false),
            None,
        );
        match action {
            RemoteHostsOverlayAction::SaveShortcut {
                label,
                connection,
                mode,
            } => {
                assert_eq!(label, "live-a");
                assert_eq!(connection.port, Some(22429));
                assert_eq!(mode, RemoteConnectMode::Vnc);
            }
            RemoteHostsOverlayAction::None
            | RemoteHostsOverlayAction::Cancelled
            | RemoteHostsOverlayAction::Open { .. }
            | RemoteHostsOverlayAction::SetDefaultWorkspace(_) => panic!("expected a shortcut action"),
        }
        let output = show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        let seen: Vec<_> = {
            let mut texts = Vec::new();
            for shape in &output.shapes {
                text_shapes(&shape.shape, &mut texts);
            }
            texts.iter().map(|text| text.galley.text().to_string()).collect()
        };
        assert!(
            !seen.iter().any(|text| text == "Save VNC shortcut"),
            "menu closed after the pick"
        );
    }

    #[test]
    fn the_card_stays_above_the_backdrop_after_a_backdrop_dismissal() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let catalog = RemoteHostCatalog {
            hosts: vec![remote_host("live-a", 22429)],
            refreshed_at: None,
        };
        let workspaces = Vec::new();
        let mut first = RemoteHostsOverlay::new();
        // Past the 200 ms click guard so the backdrop click counts.
        first.opened_at = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .expect("the process started more than a second ago");
        show_overlay(&ctx, &mut first, &catalog, &workspaces, Vec::new());
        show_overlay(&ctx, &mut first, &catalog, &workspaces, Vec::new());
        let outside = egui::Pos2::new(4.0, 896.0);
        let mut click = click_events(outside, true);
        click.extend(click_events(outside, false));
        let (_, _, action) = show_overlay_collecting(&ctx, &mut first, &catalog, &workspaces, click, None);
        assert!(matches!(action, RemoteHostsOverlayAction::Cancelled), "{action:?}");
        drop(first);
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {}).discard_textures();

        let mut second = RemoteHostsOverlay::new();
        show_overlay(&ctx, &mut second, &catalog, &workspaces, Vec::new());
        let output = show_overlay(&ctx, &mut second, &catalog, &workspaces, Vec::new());
        let row = text_center(&output, "live-a");
        let (_, layer) = show_overlay_probing(&ctx, &mut second, &catalog, &workspaces, Vec::new(), Some(row));
        assert_eq!(
            layer.expect("a layer under the card").id,
            egui::Id::new("remote_hosts_modal"),
            "the reopened card must sit above the backdrop"
        );
    }

    #[test]
    fn a_right_click_on_the_expand_chevron_opens_the_same_row_menu() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let catalog = RemoteHostCatalog {
            hosts: vec![remote_host("live-a", 22429), remote_host("live-b", 22429)],
            refreshed_at: None,
        };
        let workspaces = Vec::new();
        let mut overlay = RemoteHostsOverlay::new();
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        let output = show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        // The second row's chevron: the ">" painted left of its alias.
        let alias = text_center(&output, "live-b");
        let mut texts = Vec::new();
        for shape in &output.shapes {
            text_shapes(&shape.shape, &mut texts);
        }
        let chevron = texts
            .iter()
            .filter(|text| text.galley.text() == ">")
            .map(|text| text.pos + text.galley.size() * 0.5)
            .find(|pos| (pos.y - alias.y).abs() < 2.0)
            .expect("chevron on the second row");
        assert!(chevron.x < alias.x);

        let mut right_click = button_events(chevron, egui::PointerButton::Secondary, true);
        right_click.extend(button_events(chevron, egui::PointerButton::Secondary, false));
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, right_click);
        let output = show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        assert_eq!(overlay.selected, 1, "the right click selects the chevron's row");
        assert!(!overlay.is_expanded(&catalog.hosts[1]), "a right click does not expand");
        text_center(&output, "Save VNC shortcut");

        // The gap left of the chevron is painted row too.
        let escape = vec![key_event(egui::Key::Escape, egui::Modifiers::NONE)];
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, escape);
        let mut overlay = RemoteHostsOverlay::new();
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        let gap = egui::Pos2::new(chevron.x - 12.0, chevron.y);
        let mut right_click = button_events(gap, egui::PointerButton::Secondary, true);
        right_click.extend(button_events(gap, egui::PointerButton::Secondary, false));
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, right_click);
        let output = show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        assert_eq!(overlay.selected, 1, "the right click in the gap selects the row");
        text_center(&output, "Save VNC shortcut");
    }

    #[test]
    fn an_open_row_menu_disappears_when_the_filter_drops_its_host() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let catalog = RemoteHostCatalog {
            hosts: vec![remote_host("live-a", 22429), remote_host("live-b", 22429)],
            refreshed_at: None,
        };
        let workspaces = Vec::new();
        let mut overlay = RemoteHostsOverlay::new();
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        let output = show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        let row = text_center(&output, "live-a");
        let mut right_click = button_events(row, egui::PointerButton::Secondary, true);
        right_click.extend(button_events(row, egui::PointerButton::Secondary, false));
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, right_click);
        let output = show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        text_center(&output, "Save VNC shortcut");

        // The filter now leaves only live-b, which takes the first row.
        overlay.query = "live-b".into();
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        let output = show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        let mut texts = Vec::new();
        for shape in &output.shapes {
            text_shapes(&shape.shape, &mut texts);
        }
        assert!(
            !texts.iter().any(|text| text.galley.text() == "Save VNC shortcut"),
            "the menu must not survive on another host's row"
        );
    }

    #[test]
    fn picker_stays_above_the_card_after_the_overlay_was_dismissed_with_it_open() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let catalog = RemoteHostCatalog {
            hosts: vec![remote_host("live-a", 22429)],
            refreshed_at: None,
        };
        let workspaces = vec![WorkspaceOption {
            id: WorkspaceId(7),
            name: "Ops".into(),
        }];
        let open_picker = |overlay: &mut RemoteHostsOverlay| {
            show_overlay(&ctx, overlay, &catalog, &workspaces, Vec::new());
            let output = show_overlay(&ctx, overlay, &catalog, &workspaces, Vec::new());
            let picker = text_center(&output, "Remote Sessions (new)  \u{25be}");
            // Press and release in one frame, as a slow display delivers them:
            // the press raises the card in the same pass that opens the popup.
            let mut click = click_events(picker, true);
            click.extend(click_events(picker, false));
            show_overlay(&ctx, overlay, &catalog, &workspaces, click);
            // The popup's first frame is a sizing pass.
            let output = show_overlay(&ctx, overlay, &catalog, &workspaces, Vec::new());
            text_center(&output, "Ops")
        };

        let mut first = RemoteHostsOverlay::new();
        let ops = open_picker(&mut first);
        // Escape dismisses the overlay while its popup is still open.
        let escape = vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        show_overlay(&ctx, &mut first, &catalog, &workspaces, escape);
        drop(first);
        // A frame without the overlay, as the app renders after dismissing it.
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {}).discard_textures();

        let mut second = RemoteHostsOverlay::new();
        let ops_again = open_picker(&mut second);
        assert_eq!(ops, ops_again);
        let (_, layer) = show_overlay_probing(&ctx, &mut second, &catalog, &workspaces, Vec::new(), Some(ops));
        let layer = layer.expect("a layer under the popup entry");
        assert_ne!(
            layer.id,
            egui::Id::new("remote_hosts_modal"),
            "popup entry hidden below the card"
        );
        assert_ne!(
            layer.id,
            egui::Id::new("remote_hosts_backdrop"),
            "popup entry hidden below the backdrop"
        );
        show_overlay(&ctx, &mut second, &catalog, &workspaces, click_events(ops, true));
        show_overlay(&ctx, &mut second, &catalog, &workspaces, click_events(ops, false));
        show_overlay(&ctx, &mut second, &catalog, &workspaces, Vec::new());
        assert_eq!(second.destination, WorkspaceChoice::Existing(WorkspaceId(7)));
    }

    #[test]
    fn enter_opens_the_selected_host_with_the_current_mode_and_destination() {
        let host = remote_host("live-a", 22429);
        let mut overlay = RemoteHostsOverlay::new();
        overlay.mode = RemoteConnectMode::Vnc;
        overlay.destination = WorkspaceChoice::Existing(WorkspaceId(4));

        match overlay.open_action(&host, Some("ops")) {
            RemoteHostsOverlayAction::Open {
                label,
                connection,
                mode,
                destination,
            } => {
                assert_eq!(label, "live-a");
                assert_eq!(connection.user.as_deref(), Some("ops"));
                assert_eq!(connection.port, Some(22429));
                assert_eq!(mode, RemoteConnectMode::Vnc);
                assert_eq!(destination, WorkspaceChoice::Existing(WorkspaceId(4)));
            }
            RemoteHostsOverlayAction::None
            | RemoteHostsOverlayAction::Cancelled
            | RemoteHostsOverlayAction::SetDefaultWorkspace(_)
            | RemoteHostsOverlayAction::SaveShortcut { .. } => panic!("expected an open action"),
        }
    }

    #[test]
    fn the_row_menu_saves_a_shortcut_with_the_user_override_or_opens_in_the_header_destination() {
        let host = remote_host("live-a", 22429);
        let mut overlay = RemoteHostsOverlay::new();
        overlay.destination = WorkspaceChoice::Existing(WorkspaceId(3));

        match overlay.menu_action(&host, Some("ops"), RowMenuChoice::SaveShortcut(RemoteConnectMode::Vnc)) {
            RemoteHostsOverlayAction::SaveShortcut {
                label,
                connection,
                mode,
            } => {
                assert_eq!(label, "live-a");
                assert_eq!(connection.user.as_deref(), Some("ops"));
                assert_eq!(connection.port, Some(22429));
                assert_eq!(mode, RemoteConnectMode::Vnc);
            }
            RemoteHostsOverlayAction::None
            | RemoteHostsOverlayAction::Cancelled
            | RemoteHostsOverlayAction::Open { .. }
            | RemoteHostsOverlayAction::SetDefaultWorkspace(_) => panic!("expected a shortcut action"),
        }
        match overlay.menu_action(&host, None, RowMenuChoice::Open(RemoteConnectMode::Vnc)) {
            RemoteHostsOverlayAction::Open { mode, destination, .. } => {
                assert_eq!(
                    mode,
                    RemoteConnectMode::Vnc,
                    "the menu's mode wins over the header toggle"
                );
                assert_eq!(destination, WorkspaceChoice::Existing(WorkspaceId(3)));
            }
            RemoteHostsOverlayAction::None
            | RemoteHostsOverlayAction::Cancelled
            | RemoteHostsOverlayAction::SetDefaultWorkspace(_)
            | RemoteHostsOverlayAction::SaveShortcut { .. } => panic!("expected an open action"),
        }
    }

    #[test]
    fn set_default_is_refused_for_a_workspace_that_shares_its_name() {
        let ctx = egui::Context::default();
        let catalog = RemoteHostCatalog {
            hosts: vec![remote_host("live-a", 22429)],
            refreshed_at: None,
        };
        let workspaces = vec![
            WorkspaceOption {
                id: WorkspaceId(1),
                name: "Ops".into(),
            },
            WorkspaceOption {
                id: WorkspaceId(2),
                name: "Ops".into(),
            },
        ];
        let mut overlay = RemoteHostsOverlay::new();
        show_overlay(&ctx, &mut overlay, &catalog, &workspaces, Vec::new());
        overlay.destination = WorkspaceChoice::Existing(WorkspaceId(2));
        let (_, _, action) = show_overlay_collecting(
            &ctx,
            &mut overlay,
            &catalog,
            &workspaces,
            vec![key_event(egui::Key::D, egui::Modifiers::ALT)],
            None,
        );
        assert!(
            matches!(action, RemoteHostsOverlayAction::None),
            "an ambiguous name cannot become the default: {action:?}"
        );

        let unique = vec![WorkspaceOption {
            id: WorkspaceId(2),
            name: "Ops".into(),
        }];
        show_overlay(&ctx, &mut overlay, &catalog, &unique, Vec::new());
        overlay.destination = WorkspaceChoice::Existing(WorkspaceId(2));
        let (_, _, action) = show_overlay_collecting(
            &ctx,
            &mut overlay,
            &catalog,
            &unique,
            vec![key_event(egui::Key::D, egui::Modifiers::ALT)],
            None,
        );
        assert!(
            matches!(action, RemoteHostsOverlayAction::SetDefaultWorkspace(ref name) if name == "Ops"),
            "{action:?}"
        );
    }

    #[test]
    fn a_notice_replaces_the_host_count_until_it_expires() {
        let mut overlay = RemoteHostsOverlay::new();
        assert_eq!(overlay.current_notice(), None);
        overlay.set_notice("Saved shortcut");
        assert_eq!(overlay.current_notice(), Some("Saved shortcut"));
        let stale = Instant::now()
            .checked_sub(NOTICE_DURATION * 2)
            .expect("the process started more than eight seconds ago");
        overlay.notice = Some(("stale".into(), stale));
        assert_eq!(overlay.current_notice(), None);
    }

    #[test]
    fn a_held_tab_toggles_the_mode_once() {
        let mut keys = KeyPresses::default();
        keys.record(egui::Key::Tab, false, false);
        keys.record(egui::Key::Tab, false, true);
        keys.record(egui::Key::Tab, false, true);
        assert!(keys.tab);
        let mut repeats_only = KeyPresses::default();
        repeats_only.record(egui::Key::Tab, false, true);
        assert!(!repeats_only.tab, "an auto-repeat alone never toggles");
        repeats_only.record(egui::Key::ArrowDown, false, true);
        assert!(repeats_only.down, "arrows keep repeating");
    }

    #[test]
    fn tab_toggles_between_ssh_and_vnc() {
        assert_eq!(RemoteConnectMode::default(), RemoteConnectMode::Ssh);
        assert_eq!(RemoteConnectMode::Ssh.toggled(), RemoteConnectMode::Vnc);
        assert_eq!(RemoteConnectMode::Vnc.toggled(), RemoteConnectMode::Ssh);
        assert_eq!(RemoteConnectMode::Vnc.label(), "VNC");
    }

    #[test]
    fn expanded_host_identity_keeps_duplicate_connection_rows_separate() {
        let live_a = remote_host("live-a", 22429);
        let live_b = remote_host("live-b", 22429);
        let fresh_a = remote_host("fresh-a", 22431);
        let mut overlay = RemoteHostsOverlay::new();

        overlay.toggle_expanded(&live_a);

        assert!(overlay.is_expanded(&live_a));
        assert!(!overlay.is_expanded(&live_b));
        assert!(!overlay.is_expanded(&fresh_a));
    }

    #[test]
    fn toggle_expanded_collapses_the_same_row() {
        let live_a = remote_host("live-a", 22429);
        let mut overlay = RemoteHostsOverlay::new();

        overlay.toggle_expanded(&live_a);
        assert!(overlay.is_expanded(&live_a));

        overlay.toggle_expanded(&live_a);
        assert!(!overlay.is_expanded(&live_a));
    }

    fn remote_host(label: &str, port: u16) -> RemoteHost {
        RemoteHost {
            label: label.to_string(),
            ssh_connection: SshConnection {
                host: "127.0.0.1".to_string(),
                port: Some(port),
                user: Some("fintermac".to_string()),
                ..SshConnection::default()
            },
            sources: RemoteHostSources {
                ssh_config: true,
                tailscale: false,
            },
            status: RemoteHostStatus::Unknown,
            last_seen_secs: None,
            os: None,
            hostname: None,
            tags: Vec::new(),
            ips: Vec::new(),
        }
    }
}

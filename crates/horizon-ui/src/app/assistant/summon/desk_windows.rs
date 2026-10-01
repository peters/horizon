//! The panels of every workspace as native windows (desktop-workspace prototype).
//!
//! Each visible panel gets a window of its own with no Horizon background around
//! it. The desktop owns where the window is: when the person drags one to another
//! desktop workspace, the panel follows into that Horizon workspace.

use std::time::Duration;

use egui::{Context, Id, ViewportBuilder, ViewportId};
use horizon_core::PanelId;

use super::HorizonApp;
use crate::app::desk::{DeskState, Placement, panel_app_id};

const WINDOW_MARGIN: [f32; 2] = [28.0, 60.0];
/// How long a window is kept being sent to its desktop before giving up on it.
const GIVE_UP: Duration = Duration::from_secs(20);

impl HorizonApp {
    /// Shows every panel in a window of its own.
    pub(super) fn render_panel_windows(&mut self, ctx: &Context) {
        let windows: Vec<(PanelId, String, String, [f32; 2])> = self
            .board
            .panels
            .iter()
            .filter(|panel| panel.visible && !panel.is_assistant())
            .map(|panel| {
                (
                    panel.id,
                    panel.local_id.clone(),
                    panel.display_title().into_owned(),
                    panel.layout.size,
                )
            })
            .collect();
        for (panel_id, local_id, title, size) in windows {
            let builder = ViewportBuilder::default()
                .with_title(title)
                .with_app_id(panel_app_id(&local_id))
                .with_decorations(true)
                .with_inner_size([size[0].clamp(360.0, 1700.0), size[1].clamp(220.0, 900.0)])
                .with_min_inner_size([280.0, 160.0])
                .with_resizable(true);
            let viewport = ViewportId(Id::new(("panel_window", local_id)));
            ctx.show_viewport_immediate(viewport, builder, |ui, _class| {
                self.render_panel_window(ui, panel_id);
            });
        }
    }

    /// Puts new windows on their workspace's desktop, and moves a panel into the Horizon
    /// workspace of the desktop the person dragged its window to.
    pub(super) fn sync_panel_windows(&mut self, desk_state: &DeskState) {
        let order: Vec<_> = self.board.workspaces.iter().map(|workspace| workspace.id).collect();
        let mut moves = Vec::new();
        let panels: Vec<_> = self
            .board
            .panels
            .iter()
            .filter(|panel| panel.visible && !panel.is_assistant())
            .map(|panel| (panel.id, panel.local_id.clone(), panel.workspace_id))
            .collect();
        for (panel_id, local_id, workspace_id) in panels {
            let app_id = panel_app_id(&local_id);
            let Some(window) = desk_state.windows.iter().find(|window| window.app_id == app_id) else {
                continue;
            };
            let Some(wanted) = order.iter().position(|id| *id == workspace_id) else {
                continue;
            };
            let rect = self.initial_window_rect(panel_id);
            let Some(desk) = self.assistant.desk.as_mut() else {
                return;
            };
            let reported = usize::try_from(window.workspace).ok();
            match desk.placed.get(&panel_id).copied() {
                None => {
                    desk.placed.insert(
                        panel_id,
                        Placement {
                            since: std::time::Instant::now(),
                            confirmed: false,
                        },
                    );
                    desk.move_window(&app_id, wanted);
                    desk.place_window(&app_id, rect);
                }
                Some(placement) if !placement.confirmed => {
                    if reported == Some(wanted) {
                        desk.placed.insert(
                            panel_id,
                            Placement {
                                confirmed: true,
                                ..placement
                            },
                        );
                    } else if placement.since.elapsed() < GIVE_UP {
                        // Windows appear a little after they are created, and the desktop may not exist yet.
                        desk.move_window(&app_id, wanted);
                        desk.place_window(&app_id, rect);
                    }
                }
                Some(_) => {
                    // Anything else is the person's doing: they moved the window to another desktop.
                    if let Some(index) = reported
                        && index != wanted
                        && let Some(target) = order.get(index)
                    {
                        moves.push((panel_id, *target));
                    }
                }
            }
        }
        for (panel_id, workspace_id) in moves {
            self.board.assign_panel_to_workspace(panel_id, workspace_id);
        }
    }

    /// Where a panel's window starts: its place in the workspace, kept on the monitor.
    fn initial_window_rect(&self, panel_id: PanelId) -> [i32; 4] {
        let Some(panel) = self.board.panel(panel_id) else {
            return [0, 32, 800, 500];
        };
        let (mut left, mut top) = (f32::MAX, f32::MAX);
        for sibling in self
            .board
            .panels
            .iter()
            .filter(|other| other.workspace_id == panel.workspace_id && other.visible && !other.is_assistant())
        {
            left = left.min(sibling.layout.position[0]);
            top = top.min(sibling.layout.position[1]);
        }
        let monitor = super::desk_bar::MONITOR;
        let width = panel.layout.size[0].clamp(360.0, monitor[0] - 2.0 * WINDOW_MARGIN[0]);
        let height = panel.layout.size[1].clamp(220.0, 900.0);
        let x = (WINDOW_MARGIN[0] + panel.layout.position[0] - left).clamp(0.0, monitor[0] - width);
        let y = (WINDOW_MARGIN[1] + panel.layout.position[1] - top).clamp(32.0, monitor[1] - 120.0);
        [round(x), round(y), round(width), round(height)]
    }
}

#[allow(clippy::cast_possible_truncation)]
fn round(value: f32) -> i32 {
    value.round() as i32
}

//! Scoping the assistant to a workspace, in one click.
//!
//! A row of chips: All, Follow (the workspace the person is looking at), and the workspaces that have
//! something in them, each with a dot for what its agents are doing. Click one to look at only that
//! workspace; click it again to go back to all; hold Ctrl or Shift to add several.

use egui::{Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Ui, vec2};
use horizon_core::WorkspaceId;
use horizon_core::browser::manifest::agent_panels::AgentState;

use super::super::HorizonApp;
use crate::theme;

/// Workspaces shown as chips before the rest go behind "+N".
const SHOWN: usize = 6;

impl HorizonApp {
    /// Keeps the scope on the workspace the person is looking at while "Follow" is on.
    pub(super) fn follow_scope(&mut self) {
        if !self.assistant.summon.scope_follow {
            return;
        }
        let Some(active) = self.board.active_workspace else {
            return;
        };
        if let Some(local_id) = self
            .board
            .workspaces
            .iter()
            .find(|workspace| workspace.id == active)
            .map(|workspace| workspace.local_id.clone())
        {
            self.assistant.scope.set_only(&local_id);
        }
    }

    /// The colour of what a workspace's agents are doing: red if one waits for the person, yellow if
    /// one works, green if they are all ready. `None` when it has no agent.
    fn workspace_dot(&self, id: WorkspaceId) -> Option<Color32> {
        let mut dot = None;
        for panel in self
            .board
            .panels
            .iter()
            .filter(|panel| panel.workspace_id == id && panel.kind.is_agent() && !panel.is_assistant())
        {
            match self.board.agent_state(panel.id) {
                Some(AgentState::NeedsInput) => return Some(theme::PALETTE_RED()),
                Some(AgentState::Working) => dot = Some(theme::PALETTE_YELLOW()),
                Some(_) if dot.is_none() => dot = Some(theme::PALETTE_GREEN()),
                _ => {}
            }
        }
        dot
    }

    /// The chips, wrapped to the width they are given.
    pub(in crate::app::assistant::summon) fn scope_strip(&mut self, ui: &mut Ui) {
        let every = self.workspace_local_ids();
        let multi = ui.input(|input| input.modifiers.command || input.modifiers.shift);
        let all = self.assistant.scope.is_all() && !self.assistant.summon.scope_follow;
        let follow = self.assistant.summon.scope_follow;
        let mut after: Option<Pressed> = None;
        let mut more_anchor: Option<Rect> = None;

        let workspaces: Vec<(String, String, WorkspaceId, usize)> = self
            .board
            .workspaces
            .iter()
            .map(|workspace| {
                let count = workspace
                    .panels
                    .iter()
                    .filter(|id| self.board.panel(**id).is_some_and(|panel| !panel.is_assistant()))
                    .count();
                (workspace.local_id.clone(), workspace.name.clone(), workspace.id, count)
            })
            .filter(|(_, _, _, count)| *count > 0)
            .collect();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if chip(ui, "All", all, None).clicked() {
                after = Some(Pressed::All);
            }
            if chip(ui, "Follow me", follow, None)
                .on_hover_text("Look at the workspace you are in")
                .clicked()
            {
                after = Some(Pressed::Follow);
            }
            for (local_id, name, id, _) in workspaces.iter().take(SHOWN) {
                let selected = !all && !follow && self.assistant.scope.includes(local_id);
                let selected = selected || (follow && self.assistant.scope.includes(local_id));
                if chip(ui, name, selected, self.workspace_dot(*id)).clicked() {
                    after = Some(Pressed::Workspace(local_id.clone()));
                }
            }
            if workspaces.len() > SHOWN {
                let response = chip(ui, &format!("+{}", workspaces.len() - SHOWN), false, None);
                more_anchor = Some(response.rect);
                if response.clicked() {
                    after = Some(Pressed::More);
                }
            }
        });
        match after {
            Some(Pressed::All) => {
                self.assistant.scope.set_all();
                self.assistant.summon.scope_follow = false;
            }
            Some(Pressed::Follow) => self.assistant.summon.scope_follow = !follow,
            Some(Pressed::Workspace(local_id)) => {
                self.assistant.summon.scope_follow = false;
                if multi {
                    self.assistant.scope.toggle(&local_id, &every);
                } else if self.assistant.scope.count() == Some(1) && self.assistant.scope.includes(&local_id) {
                    self.assistant.scope.set_all();
                } else {
                    self.assistant.scope.set_only(&local_id);
                }
            }
            Some(Pressed::More) => {
                self.assistant.scope_open = !self.assistant.scope_open;
                self.assistant.scope_anchor = more_anchor;
            }
            None => {}
        }
    }
}

enum Pressed {
    All,
    Follow,
    Workspace(String),
    More,
}

/// One chip: a pill with an optional state dot.
fn chip(ui: &mut Ui, text: &str, selected: bool, dot: Option<Color32>) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(12.0), theme::FG());
    let dot_room = if dot.is_some() { 14.0 } else { 0.0 };
    let size = vec2(galley.size().x + 22.0 + dot_room, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let hovered = response.hovered();
    ui.painter().rect(
        rect,
        CornerRadius::same(99),
        if selected {
            theme::ACCENT().gamma_multiply(0.26)
        } else if hovered {
            theme::PANEL_BG_ALT()
        } else {
            theme::PANEL_BG()
        },
        Stroke::new(
            1.0,
            if selected {
                theme::ACCENT()
            } else {
                theme::BORDER_SUBTLE()
            },
        ),
        StrokeKind::Inside,
    );
    let mut x = rect.left() + 11.0;
    if let Some(color) = dot {
        ui.painter()
            .circle_filled(egui::pos2(x + 3.5, rect.center().y), 3.5, color);
        x += dot_room;
    }
    ui.painter().galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        if selected { theme::FG() } else { theme::FG_SOFT() },
    );
    response
}

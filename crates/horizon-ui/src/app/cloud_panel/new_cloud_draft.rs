//! The New cloud draft that a dismissal leaves in its workspace: a card shows where the
//! dialog went, and brings it back with every choice still in it. Only Cancel ends a draft.
use super::HorizonApp;
use crate::app::view::canvas_scene_transform;
use crate::theme;
use egui::{Id, Order, Pos2, Rect, RichText, Stroke, StrokeKind, Vec2, vec2};

/// The card's size in canvas units: a title, two lines and its two actions.
const CARD: Vec2 = Vec2::new(360.0, 152.0);

/// Where a dismissed New cloud dialog waits.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct Draft {
    /// The workspace whose cloud this draft makes, by local id; none for a dialog that a
    /// workspace did not open, which `+ New cloud` brings back.
    pub(in crate::app) workspace: Option<String>,
    /// Where the card sits in that workspace, from the workspace's top-left corner.
    pub(in crate::app) position: [f32; 2],
    /// A card that a saved record brought back: its dialog has read nothing in this session,
    /// so Continue reads its repository before it shows the choices again.
    pub(in crate::app) restored: bool,
}

impl HorizonApp {
    /// Takes the dialog out of view and leaves its draft in its workspace, where the card
    /// brings it back. Nothing typed, chosen, checked, cloned or started is lost: only
    /// Cancel ends a draft.
    pub(in crate::app) fn hide_cloud_creation(&mut self, ctx: &egui::Context) {
        if self.cloud_prototype.production.draft.is_some() {
            return;
        }
        let workspace = self.cloud_prototype.production.creation_workspace();
        self.dir_picker = None;
        // Beside the clouds already in that workspace, so the card never sits on one.
        let position = workspace.as_ref().map_or([0.0, 0.0], |workspace| {
            self.cloud_prototype.groups.next_position(workspace, &self.board)
        });
        let workspace = workspace.filter(|local| {
            // The card belongs to that workspace, so the person sees it land there. A detached
            // workspace shows it again when it rejoins the board, which takes no view change.
            self.board
                .workspace_id_by_local_id(local)
                .is_some_and(|id| self.workspace_is_detached(id) || self.focus_workspace_visible(ctx, id, false))
        });
        self.cloud_prototype.production.draft = Some(Draft {
            workspace,
            position,
            restored: false,
        });
        // The card outlives this session, so a worker thread saves its choices beside the
        // cloud settings and this frame keeps them in memory.
        self.keep_cloud_draft_record(ctx);
        ctx.request_repaint();
    }

    /// Shows the dialog of a draft again, in its workspace, with every choice it kept.
    /// False when nothing waits.
    pub(in crate::app) fn reopen_cloud_draft(&mut self, ctx: &egui::Context) -> bool {
        let Some(draft) = self.cloud_prototype.production.draft.take() else {
            return false;
        };
        self.cloud_prototype.production.creating = true;
        self.cloud_prototype.production.focus_title_on_open = true;
        // A restored draft reads its repository only now: the choices it kept, such as the
        // profile, apply to the settings that arrive.
        if draft.restored {
            self.read_restored_cloud_draft(ctx);
        }
        if let Some(workspace) = draft
            .workspace
            .as_deref()
            .and_then(|workspace| self.board.workspace_id_by_local_id(workspace))
        {
            self.board.focus_workspace(workspace);
        }
        ctx.request_repaint();
        true
    }

    /// Draws the card of a dismissed draft where its workspace is on the canvas, and lets a
    /// repository check that the hidden dialog started land in it.
    pub(in crate::app) fn render_new_cloud_draft(&mut self, ctx: &egui::Context) {
        let Some(draft) = self.cloud_prototype.production.draft.clone() else {
            return;
        };
        if self.cloud_prototype.fullscreen.is_some() {
            return;
        }
        // A check of the hidden dialog still ends here: it adds the cloud's card and takes
        // the view to it. A stock watch and an image publishing form go on too.
        self.poll_parked_cloud_draft(ctx);
        let Some(local) = draft.workspace.as_deref() else {
            // A dialog that no workspace opened has no card; `+ New cloud` brings it back.
            return;
        };
        let Some(workspace) = self.board.workspace_id_by_local_id(local) else {
            // The workspace went with a session or a removal: the draft has no place.
            self.cloud_prototype.production.draft = None;
            return;
        };
        if self.workspace_is_detached(workspace) {
            return;
        }
        let Some(origin) = self
            .board
            .workspaces
            .iter()
            .find(|item| item.local_id == local)
            .map(|item| item.position)
        else {
            return;
        };
        let canvas = self.canvas_rect(ctx);
        let transform = canvas_scene_transform(canvas, self.canvas_view);
        let rect = Rect::from_min_size(
            Pos2::new(origin[0] + draft.position[0], origin[1] + draft.position[1]),
            CARD,
        );
        if !(transform * rect).intersects(canvas) {
            return;
        }
        let clip = transform.inverse() * canvas;
        let (title, repository, status) = self.cloud_prototype.production.draft_card();
        let mut continued = false;
        let mut cancelled = false;
        egui::Area::new(Id::unique("new-cloud-draft"))
            .order(Order::Middle)
            .fixed_pos(rect.min)
            .constrain(false)
            .show(ctx, |ui| {
                ui.ctx().set_transform_layer(ui.layer_id(), transform);
                ui.set_clip_rect(clip);
                ui.allocate_exact_size(CARD, egui::Sense::hover());
                ui.painter().rect_filled(rect, 14, theme::PANEL_BG());
                ui.painter().rect_stroke(
                    rect,
                    14,
                    Stroke::new(1.0, theme::alpha(theme::ACCENT(), 85)),
                    StrokeKind::Inside,
                );
                let mut body = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect.shrink2(vec2(18.0, 15.0)))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                body.spacing_mut().item_spacing = vec2(8.0, 5.0);
                body.label(RichText::new(title).size(16.0).color(theme::FG()));
                body.label(RichText::new(repository).size(12.5).color(theme::FG_SOFT()));
                body.label(RichText::new(status).size(12.5).color(theme::FG_DIM()));
                body.add_space(8.0);
                body.horizontal(|ui| {
                    continued = ui
                        .add(crate::app::util::primary_button("Continue").min_size(vec2(104.0, 28.0)))
                        .clicked();
                    cancelled = ui
                        .add(egui::Button::new(RichText::new("Cancel").size(13.0)).min_size(vec2(84.0, 28.0)))
                        .clicked();
                });
            });
        if continued {
            self.reopen_cloud_draft(ctx);
        } else if cancelled {
            self.close_cloud_creation_to_board(ctx);
        }
    }
}

#[cfg(test)]
mod tests;

//! Parks the terminals of a ready cloud while they are out of view, and attaches
//! them again when they come back. A parked terminal keeps no SSH client, PTY or
//! grid on this computer; its agent continues in tmux on the worker. While a cloud
//! has parked terminals, Horizon reads a short status of their sessions, which the
//! parked panels show in a strip and as their agent status.
use super::{HorizonApp, PanelKind, Stage, cloud_runtime};
use horizon_core::{
    CloudWait, PanelId, ParkedPanel,
    cloud_panel::park::{ParkAction, ParkPolicy, ParkTracker, Sight},
    cloud_runtime::session_status::{SessionActivity, SessionStatus},
};
use std::{
    collections::{HashMap, HashSet},
    sync::mpsc::Receiver,
    time::Instant,
};

use read::StatusRead;

const STRIP_HEIGHT: f32 = 22.0;

/// The park state of one cloud.
#[derive(Default)]
pub(super) struct Parking {
    /// Set when the cloud first becomes ready: attached when it was in view then.
    tracker: Option<ParkTracker>,
    /// The last status of each parked terminal, by its panel's local id.
    statuses: HashMap<String, SessionStatus>,
    reader: Option<Receiver<StatusRead>>,
    /// When the read in flight started.
    read_started: Option<Instant>,
    /// When the read that gave `statuses` started.
    statuses_since: Option<Instant>,
    next_read: Option<Instant>,
    error: Option<String>,
    policy: ParkPolicy,
}

impl Parking {
    /// Whether the terminals of the cloud are parked.
    pub(super) fn is_parked(&self) -> bool {
        self.tracker.is_some_and(|tracker| tracker.is_parked())
    }

    /// The last status of each parked terminal, by its panel's local id.
    pub(super) fn statuses(&self) -> &HashMap<String, SessionStatus> {
        &self.statuses
    }
}

/// What a parked session does, in words.
pub(super) fn activity_text(activity: SessionActivity) -> String {
    match activity {
        SessionActivity::Working => "Working".to_owned(),
        SessionActivity::Idle => "Idle".to_owned(),
        SessionActivity::Exited(Some(code)) => format!("Ended with status {code}"),
        SessionActivity::Exited(None) => "Ended".to_owned(),
        SessionActivity::Missing => "Session not found".to_owned(),
    }
}

/// What the user saw of a cloud as it became ready.
#[derive(Clone, Copy)]
pub(super) enum ReadyView {
    /// The cloud did not just become ready.
    Unchanged,
    Visible,
    /// Out of view, with the focus to give back after missing sessions are recreated.
    Hidden {
        focused: Option<PanelId>,
        workspace: Option<horizon_core::WorkspaceId>,
    },
}

/// What the user saw of a tracked cloud, and whether one of its terminals runs live.
#[derive(Clone, Copy)]
struct Observed {
    sight: Sight,
    live: bool,
}

/// Panels that a cloud can park: terminals, not browsers or desktops.
fn parks(kind: PanelKind) -> bool {
    !matches!(kind, PanelKind::Browser | PanelKind::Device)
}

impl HorizonApp {
    /// The members of cloud `index` that can park, with their panel ids.
    fn parkable_members(&self, index: usize) -> Vec<(String, PanelId)> {
        self.cloud_prototype.groups.0[index]
            .panels
            .iter()
            .filter_map(|local| {
                let id = self.board.panel_id_by_local_id(local)?;
                self.board
                    .panel(id)
                    .filter(|panel| parks(panel.kind))
                    .map(|_| (local.clone(), id))
            })
            .collect()
    }

    /// What the user saw of each tracked ready cloud in the last frame, by group
    /// index; `None` for a cloud that the tracker does not observe now. One pass over
    /// the board serves every cloud, and nothing is built while no cloud is tracked.
    fn observe_clouds(&self) -> Vec<Option<Observed>> {
        let production = &self.cloud_prototype.production;
        let groups = &self.cloud_prototype.groups.0;
        let tracked = |group: &horizon_core::cloud_panel::CloudGroup| {
            group.remote.is_some()
                && !production.closing(group.issue)
                && production.runtimes.get(&group.issue).is_some_and(|runtime| {
                    runtime.parking.tracker.is_some()
                        && !runtime.needs_attach
                        && runtime.stage == Some(Stage::Ready)
                        && runtime.state.as_ref().is_some_and(|state| state.stage == Stage::Ready)
                })
        };
        if !groups.iter().any(tracked) {
            return vec![None; groups.len()];
        }
        let panels: HashMap<&str, &horizon_core::Panel> = self
            .board
            .panels
            .iter()
            .map(|panel| (panel.local_id.as_str(), panel))
            .collect();
        groups
            .iter()
            .map(|group| {
                // Sight is tracked while attaches retry, so a cloud that leaves the view parks.
                if !tracked(group) {
                    return None;
                }
                let mut observed = Observed {
                    sight: Sight::Hidden,
                    live: false,
                };
                // Any panel of the cloud shows that the user looks at it; only terminals park.
                for panel in group.panels.iter().filter_map(|local| panels.get(local.as_str())) {
                    observed.live |= parks(panel.kind) && panel.cloud_wait().is_none();
                    if self.board.focused == Some(panel.id) || self.fullscreen_panel == Some(panel.id) {
                        observed.sight = Sight::InUse;
                    } else if observed.sight == Sight::Hidden && self.panel_drawn_last_frame(panel.id) {
                        observed.sight = Sight::Visible;
                    }
                }
                Some(observed)
            })
            .collect()
    }

    /// What the user sees of cloud `index` in the last frame.
    pub(super) fn cloud_sight(&self, index: usize) -> Sight {
        let members: Vec<PanelId> = self.cloud_prototype.groups.0[index]
            .panels
            .iter()
            .filter_map(|local| self.board.panel_id_by_local_id(local))
            .collect();
        let in_use = |id: PanelId| self.board.focused == Some(id) || self.fullscreen_panel == Some(id);
        if members.iter().any(|id| in_use(*id)) {
            Sight::InUse
        } else if members.iter().any(|id| self.panel_drawn_last_frame(*id)) {
            Sight::Visible
        } else {
            Sight::Hidden
        }
    }

    /// Whether sessions restored now for cloud `index` start parked: the cloud is
    /// out of view as it becomes ready, or its terminals are parked already.
    pub(super) fn restores_parked(&self, index: usize, view: ReadyView) -> bool {
        match view {
            ReadyView::Hidden { .. } => true,
            // A cloud in view as it becomes ready attaches, whatever it did before.
            ReadyView::Visible => false,
            ReadyView::Unchanged => self
                .cloud_prototype
                .production
                .runtimes
                .get(&self.cloud_prototype.groups.0[index].issue)
                .and_then(|runtime| runtime.parking.tracker)
                .is_some_and(|tracker| tracker.is_parked()),
        }
    }

    /// What the user sees of cloud `index` as it becomes ready. When the cloud is out
    /// of view, its terminals park instead of attaching, so a restart opens no
    /// connection for clouds that nobody looks at.
    pub(super) fn ready_view(&self, index: usize, ready_now: bool) -> ReadyView {
        if !ready_now {
            ReadyView::Unchanged
        } else if self.cloud_sight(index) == Sight::Hidden {
            ReadyView::Hidden {
                focused: self.board.focused,
                workspace: self.board.active_workspace,
            }
        } else {
            ReadyView::Visible
        }
    }

    /// Applies what the user saw of cloud `index` when it became ready. A hidden
    /// cloud gets back the focus that a recreated session took, and all its
    /// terminals park: the waiting ones leave `members`, and recreated live ones
    /// detach again.
    pub(super) fn apply_ready_view(&mut self, index: usize, view: ReadyView, members: &mut HashSet<String>) {
        let issue = self.cloud_prototype.groups.0[index].issue;
        let tracker = match view {
            ReadyView::Unchanged => return,
            ReadyView::Visible => ParkTracker::attached(),
            ReadyView::Hidden { focused, workspace } => {
                self.board.focused = focused;
                self.board.active_workspace = workspace;
                ParkTracker::parked()
            }
        };
        if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&issue) {
            runtime.parking.tracker = Some(tracker);
        }
        if !tracker.is_parked() {
            self.record_cloud_parking(index);
            return;
        }
        for (local, id) in self.parkable_members(index) {
            members.remove(&local);
            self.park_member(id);
            if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&issue) {
                runtime.pending_member_attachments.remove(&local);
            }
        }
        self.record_cloud_parking(index);
    }

    /// Records in the runtime index of a saved session whether the terminals of cloud
    /// `index` are parked, and the status that the last read returned for each. A
    /// failure is logged: the board itself does not depend on this record.
    pub(super) fn record_cloud_parking(&self, index: usize) {
        let Some(session) = self.active_session.as_ref().filter(|session| session.persistent) else {
            return;
        };
        let Some(parking) = self
            .cloud_prototype
            .production
            .runtimes
            .get(&self.cloud_prototype.groups.0[index].issue)
            .map(|runtime| &runtime.parking)
        else {
            return;
        };
        let parked = parking.tracker.is_some_and(|tracker| tracker.is_parked());
        let members = self.parkable_members(index);
        let panels: Vec<ParkedPanel<'_>> = members
            .iter()
            .map(|(local, _)| ParkedPanel {
                local_id: local,
                parked,
                status: parking.statuses.get(local),
            })
            .collect();
        if panels.is_empty() {
            return;
        }
        if let Err(error) = self.session_store.record_cloud_panels(&session.session_id, &panels) {
            tracing::warn!("could not record the park state of a cloud: {error}");
        }
    }

    fn drop_pending_terminal_attachments(&mut self, index: usize) {
        let terminals: Vec<String> = self
            .parkable_members(index)
            .into_iter()
            .map(|(local, _)| local)
            .collect();
        let issue = self.cloud_prototype.groups.0[index].issue;
        if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&issue) {
            runtime
                .pending_member_attachments
                .retain(|local| !terminals.contains(local));
        }
    }

    fn park_member(&mut self, id: PanelId) {
        let Some(panel) = self.board.panel_mut(id) else {
            return;
        };
        match panel.park_cloud() {
            Ok(_) => {
                self.panel_render_caches.terminal_grid_cache.remove(&id);
            }
            Err(error) => self.cloud_prototype.error = Some(error.to_string()),
        }
    }

    /// Attaches, restores and parks the members of each cloud, and shows why a member waits.
    pub(super) fn sync_cloud_members(&mut self) {
        self.sync_cloud_presentations();
        self.sync_cloud_member_waits();
        self.sync_cloud_parking();
    }

    /// Parks or attaches the terminals of each ready cloud as its sight changes, and
    /// reads the status of parked sessions. At most one cloud attaches per frame, so
    /// a view that shows many clouds at once opens their connections one by one.
    pub(super) fn sync_cloud_parking(&mut self) {
        let now = Instant::now();
        let mut attached = false;
        let observed = self.observe_clouds();
        for (index, observed) in observed.into_iter().enumerate() {
            let Some(Observed { sight, live }) = observed else {
                continue;
            };
            let issue = self.cloud_prototype.groups.0[index].issue;
            let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&issue) else {
                continue;
            };
            let Some(tracker) = runtime.parking.tracker.as_mut() else {
                continue;
            };
            // A hidden cloud is still observed, so a fast pan resets its dwell.
            if attached && tracker.is_parked() && sight != Sight::Hidden {
                continue;
            }
            let action = tracker.observe(sight, now, runtime.parking.policy);
            // A terminal that started while its cloud was parked, such as a session restored
            // at Ready, parks with the others while the cloud stays out of view.
            let park_live = live && action.is_none() && tracker.is_parked() && sight == Sight::Hidden;
            if tracker.is_parked() && sight == Sight::Hidden && !runtime.pending_member_attachments.is_empty() {
                // A retry of an attach that failed waits for the cloud to come into view again.
                self.drop_pending_terminal_attachments(index);
            }
            match action {
                Some(ParkAction::Park) => {
                    for (_, id) in self.parkable_members(index) {
                        self.park_member(id);
                    }
                    if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&issue) {
                        runtime.parking.next_read = Some(now);
                    }
                    self.record_cloud_parking(index);
                }
                Some(ParkAction::Attach) => {
                    attached = true;
                    let parked: Vec<String> = self
                        .parkable_members(index)
                        .into_iter()
                        .filter(|(_, id)| self.board.panel(*id).is_some_and(|panel| panel.cloud_wait().is_some()))
                        .map(|(local, _)| local)
                        .collect();
                    if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&issue) {
                        // The attach path that restores members after Ready retries a failed attach.
                        runtime.pending_member_attachments.extend(parked);
                        runtime.next_attachment_attempt = None;
                        runtime.parking.statuses.clear();
                        runtime.parking.statuses_since = None;
                        runtime.parking.error = None;
                        // A read in flight answers for terminals that are attaching now.
                        runtime.parking.reader = None;
                        // The restore runs in the next frame; an idle board would wait for its poll.
                        if let Some(context) = &runtime.repaint_context {
                            context.request_repaint();
                        }
                    }
                    self.record_cloud_parking(index);
                    continue;
                }
                None if park_live => {
                    let live: Vec<PanelId> = self
                        .parkable_members(index)
                        .into_iter()
                        .filter(|(_, id)| self.board.panel(*id).is_some_and(|panel| panel.cloud_wait().is_none()))
                        .map(|(_, id)| id)
                        .collect();
                    for id in live {
                        self.park_member(id);
                    }
                }
                None => {}
            }
            self.read_parked_status(index, now);
        }
    }

    /// The strip text of a parked panel.
    fn parked_strip_text(&self, local: &str) -> String {
        let runtime = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.panels.iter().any(|member| member == local))
            .and_then(|group| self.cloud_prototype.production.runtimes.get(&group.issue));
        let status = runtime.and_then(|runtime| runtime.parking.statuses.get(local));
        let error = runtime.and_then(|runtime| runtime.parking.error.as_deref());
        match (status, error) {
            (Some(status), _) => {
                let activity = activity_text(status.activity);
                match status.last_line() {
                    Some(line) => format!("Parked · {activity} · {line}"),
                    None => format!("Parked · {activity}"),
                }
            }
            (None, Some(_)) => "Parked · status unavailable".to_owned(),
            (None, None) => "Parked · the agent continues on the worker".to_owned(),
        }
    }

    /// Draws a status strip at the bottom of each parked terminal on the canvas, in
    /// the panel's own layer so menus and dialogs stay above it.
    pub(in crate::app) fn render_parked_strips(&self, ctx: &egui::Context) {
        let canvas = self.canvas_rect(ctx);
        for panel in self
            .board
            .panels
            .iter()
            .filter(|panel| panel.cloud_wait() == Some(CloudWait::Parked))
        {
            let id = panel.id;
            let Some(&body) = self.terminal_body_screen_rects.get(&id) else {
                continue;
            };
            let order = if self.board.focused == Some(id) {
                egui::Order::Foreground
            } else {
                egui::Order::Middle
            };
            // The panel layer carries the canvas transform, so the strip is drawn in
            // canvas coordinates and sized to stay the same on the screen.
            let to_canvas = crate::app::view::canvas_scene_transform(canvas, self.canvas_view).inverse();
            let scale = 1.0 / self.canvas_view.zoom.max(f32::EPSILON);
            let body = to_canvas * body;
            // The clip is set, not intersected with the default screen clip: a body in
            // canvas coordinates can lie far outside the screen rectangle.
            let painter = egui::Painter::new(
                ctx.clone(),
                egui::LayerId::new(order, egui::Id::new(("panel", id.0))),
                body,
            );
            let strip =
                egui::Rect::from_min_max(egui::pos2(body.left(), body.bottom() - STRIP_HEIGHT * scale), body.max);
            painter.rect_filled(strip, 0.0, crate::theme::alpha(crate::theme::PANEL_BG_ALT(), 235));
            painter.text(
                strip.left_center() + egui::vec2(8.0 * scale, 0.0),
                egui::Align2::LEFT_CENTER,
                self.parked_strip_text(&panel.local_id),
                egui::FontId::proportional(12.0 * scale),
                crate::theme::FG_SOFT(),
            );
        }
    }
}

mod read;
#[cfg(all(test, unix))]
pub(super) mod tests;

//! Parks the terminals of a ready cloud while they are out of view, and attaches
//! them again when they come back. A parked terminal keeps no SSH client, PTY or
//! grid on this computer; its agent continues in tmux on the worker. While a cloud
//! has parked terminals, Horizon reads a short status of their sessions, which the
//! parked panels show in a strip and as their agent status.
use super::{HorizonApp, PanelKind, Settings, Stage, cloud_runtime};
use horizon_core::{
    AgentStatus, CloudWait, PanelId,
    cloud_panel::park::{ParkAction, ParkPolicy, ParkTracker, Sight},
    cloud_runtime::session_status::{self, SessionActivity, SessionStatus},
};
use std::{
    collections::{HashMap, HashSet},
    sync::mpsc::{Receiver, TryRecvError, channel},
    time::{Duration, Instant},
};

/// How often the sessions of parked terminals are read.
const STATUS_INTERVAL: Duration = Duration::from_secs(10);
const STRIP_HEIGHT: f32 = 22.0;

type StatusRead = cloud_runtime::Result<Vec<SessionStatus>>;

/// The park state of one cloud.
#[derive(Default)]
pub(super) struct Parking {
    /// Set when the cloud first becomes ready: attached when it was in view then.
    tracker: Option<ParkTracker>,
    /// The last status of each parked terminal, by its panel's local id.
    statuses: HashMap<String, SessionStatus>,
    reader: Option<Receiver<StatusRead>>,
    next_read: Option<Instant>,
    error: Option<String>,
    policy: ParkPolicy,
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

    /// What the user sees of cloud `index` in the last frame.
    fn cloud_sight(&self, index: usize) -> Sight {
        let members = self.parkable_members(index);
        let in_use = |id: PanelId| self.board.focused == Some(id) || self.fullscreen_panel == Some(id);
        if members.iter().any(|(_, id)| in_use(*id)) {
            Sight::InUse
        } else if members.iter().any(|(_, id)| self.panel_drawn_last_frame(*id)) {
            Sight::Visible
        } else {
            Sight::Hidden
        }
    }

    /// Called when cloud `index` becomes ready and `members` are about to attach. When
    /// the cloud is out of view, its terminals park instead and are removed from
    /// `members`, so a restart opens no connection for clouds that nobody looks at.
    pub(super) fn park_hidden_members_on_ready(&mut self, index: usize, members: &mut HashSet<String>) {
        let parked = self.cloud_sight(index) == Sight::Hidden;
        let issue = self.cloud_prototype.groups.0[index].issue;
        if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&issue) {
            runtime.parking.tracker = Some(if parked {
                ParkTracker::parked()
            } else {
                ParkTracker::attached()
            });
        }
        if !parked {
            return;
        }
        for (local, id) in self.parkable_members(index) {
            if members.remove(&local) {
                self.park_member(id);
                if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&issue) {
                    runtime.pending_member_attachments.remove(&local);
                }
            }
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

    /// Parks or attaches the terminals of each ready cloud as its sight changes, and
    /// reads the status of parked sessions. At most one cloud attaches per frame, so
    /// a view that shows many clouds at once opens their connections one by one.
    pub(super) fn sync_cloud_parking(&mut self) {
        let now = Instant::now();
        let mut attached = false;
        for index in 0..self.cloud_prototype.groups.0.len() {
            let group = &self.cloud_prototype.groups.0[index];
            if group.remote.is_none() || self.cloud_prototype.production.closing(group.issue) {
                continue;
            }
            let issue = group.issue;
            let sight = self.cloud_sight(index);
            let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&issue) else {
                continue;
            };
            let ready = runtime.state.as_ref().is_some_and(|state| state.stage == Stage::Ready)
                && runtime.stage == Some(Stage::Ready);
            if !ready || runtime.attaching() {
                continue;
            }
            let Some(tracker) = runtime.parking.tracker.as_mut() else {
                continue;
            };
            if attached && tracker.is_parked() {
                continue;
            }
            match tracker.observe(sight, now, runtime.parking.policy) {
                Some(ParkAction::Park) => {
                    for (_, id) in self.parkable_members(index) {
                        self.park_member(id);
                    }
                    if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&issue) {
                        runtime.parking.next_read = Some(now);
                    }
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
                        runtime.parking.error = None;
                    }
                }
                None => {}
            }
            self.read_parked_status(index, now);
        }
    }

    /// Starts a status read of the parked terminals of cloud `index` when one is
    /// due, and applies a finished one.
    fn read_parked_status(&mut self, index: usize, now: Instant) {
        let parked: HashMap<String, PanelId> = self
            .parkable_members(index)
            .into_iter()
            .filter(|(_, id)| {
                self.board
                    .panel(*id)
                    .is_some_and(|panel| panel.cloud_wait() == Some(CloudWait::Parked))
            })
            .collect();
        let group = &self.cloud_prototype.groups.0[index];
        let (issue, launch) = (group.issue, group.remote.clone());
        let root = self.cloud_prototype.root.clone();
        let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&issue) else {
            return;
        };
        if let Some(reader) = &runtime.parking.reader {
            match reader.try_recv() {
                Ok(result) => {
                    runtime.parking.reader = None;
                    runtime.parking.next_read = Some(now + STATUS_INTERVAL);
                    let tmux_to_local: HashMap<String, String> = runtime
                        .state
                        .iter()
                        .flat_map(|state| &state.sessions)
                        .map(|session| (session.tmux.clone(), session.panel_id.clone()))
                        .collect();
                    match result {
                        Ok(statuses) => {
                            runtime.parking.error = None;
                            runtime.parking.statuses = statuses
                                .into_iter()
                                .filter_map(|status| Some((tmux_to_local.get(&status.id)?.clone(), status)))
                                .collect();
                        }
                        Err(error) => runtime.parking.error = Some(error.to_string()),
                    }
                    let reported: Vec<(PanelId, AgentStatus)> = parked
                        .iter()
                        .filter_map(|(local, id)| {
                            let status = runtime.parking.statuses.get(local)?;
                            Some((
                                *id,
                                if status.activity == SessionActivity::Working {
                                    AgentStatus::Working
                                } else {
                                    AgentStatus::Idle
                                },
                            ))
                        })
                        .collect();
                    for (id, status) in reported {
                        if let Some(panel) = self.board.panel_mut(id) {
                            panel.set_parked_agent_status(status);
                        }
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => runtime.parking.reader = None,
            }
            return;
        }
        if parked.is_empty() || runtime.parking.next_read.is_some_and(|at| now < at) {
            return;
        }
        let ids: Vec<String> = runtime
            .state
            .iter()
            .flat_map(|state| &state.sessions)
            .filter(|session| parked.contains_key(&session.panel_id))
            .map(|session| session.tmux.clone())
            .take(session_status::MAX_SESSIONS)
            .collect();
        let (Some(worker), Some(root), Some(launch)) = (
            runtime.state.as_ref().and_then(|state| state.worker.clone()),
            root,
            launch,
        ) else {
            return;
        };
        runtime.parking.next_read = Some(now + STATUS_INTERVAL);
        if ids.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        runtime.parking.reader = Some(rx);
        let repaint = runtime.repaint_context.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let settings = Settings::load(&root.join("settings.json"))?;
                let directory = cloud_runtime::state::cloud_directory(&root, &launch.id)?;
                session_status::read(
                    &worker,
                    &settings,
                    &directory,
                    &ids,
                    &cloud_runtime::Cancellation::default(),
                )
            })();
            if tx.send(result).is_ok()
                && let Some(context) = repaint
            {
                context.request_repaint();
            }
        });
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
                let activity = match status.activity {
                    SessionActivity::Working => "Working".to_owned(),
                    SessionActivity::Idle => "Idle".to_owned(),
                    SessionActivity::Exited(Some(code)) => format!("Ended with status {code}"),
                    SessionActivity::Exited(None) => "Ended".to_owned(),
                    SessionActivity::Missing => "Session not found".to_owned(),
                };
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
        for (&id, &body) in &self.terminal_body_screen_rects {
            let Some(panel) = self
                .board
                .panel(id)
                .filter(|panel| panel.cloud_wait() == Some(CloudWait::Parked))
            else {
                continue;
            };
            let order = if self.board.focused == Some(id) {
                egui::Order::Foreground
            } else {
                egui::Order::Middle
            };
            let painter = ctx
                .layer_painter(egui::LayerId::new(order, egui::Id::new(("panel", id.0))))
                .with_clip_rect(body);
            let strip = egui::Rect::from_min_max(egui::pos2(body.left(), body.bottom() - STRIP_HEIGHT), body.max);
            painter.rect_filled(strip, 0.0, crate::theme::alpha(crate::theme::PANEL_BG_ALT(), 235));
            painter.text(
                strip.left_center() + egui::vec2(8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                self.parked_strip_text(&panel.local_id),
                egui::FontId::proportional(12.0),
                crate::theme::FG_SOFT(),
            );
        }
    }
}

#[cfg(all(test, unix))]
mod tests;

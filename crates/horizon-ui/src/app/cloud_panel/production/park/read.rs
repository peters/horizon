//! The status reads of parked terminals: a short status of each parked session,
//! read from the worker every few seconds and on request.
use super::super::{Settings, Store};
use super::{HorizonApp, Parking, cloud_runtime};
use horizon_core::{
    AgentStatus, CloudWait, PanelId,
    cloud_runtime::session_status::{self, SessionActivity, SessionStatus},
};
use std::{
    collections::HashMap,
    sync::mpsc::{TryRecvError, channel},
    time::{Duration, Instant},
};

/// How often the sessions of parked terminals are read.
const STATUS_INTERVAL: Duration = Duration::from_secs(10);

/// The statuses of a finished read, by the local id of each parked panel.
pub(super) type StatusRead = cloud_runtime::Result<Vec<(String, SessionStatus)>>;

impl Parking {
    /// Whether the statuses come from a read that started at `at` or later.
    pub(in crate::app::cloud_panel::production) fn read_since(&self, at: Instant) -> bool {
        self.statuses_since.is_some_and(|since| since >= at)
    }

    /// Makes the next read due at `now`. A read in flight finishes first.
    pub(in crate::app::cloud_panel::production) fn read_now(&mut self, now: Instant) {
        if self.reader.is_none() {
            self.next_read = Some(now);
        }
    }
}

/// The tmux session of each parked panel in `locals`, by session id, from the
/// sessions that the cloud's record holds.
pub(super) fn parked_sessions(sessions: Vec<super::super::Session>, locals: &[String]) -> HashMap<String, String> {
    sessions
        .into_iter()
        .filter(|session| locals.contains(&session.panel_id))
        .map(|session| (session.tmux, session.panel_id))
        .collect()
}

/// Reads the status of the parked panels `locals` of the cloud `cloud_id` on the worker of `deployment`.
fn read_statuses(
    deployment: &super::super::Deployment,
    root: &std::path::Path,
    cloud_id: &str,
    locals: &[String],
) -> StatusRead {
    let settings = Settings::load(&root.join("settings.json"))?;
    let directory = cloud_runtime::state::cloud_directory(root, cloud_id)?;
    // The saved record, not the copy from when the cloud became ready: a panel opened
    // since then records its session only there.
    let sessions = Store::lock(&directory)?
        .load()?
        .map(|state| state.sessions)
        .unwrap_or_default();
    let tmux = parked_sessions(sessions, locals);
    if tmux.is_empty() {
        return Ok(every_local(locals, &tmux, Vec::new()));
    }
    let ids: Vec<String> = tmux.keys().cloned().collect();
    let worker = deployment
        .worker
        .as_ref()
        .ok_or(cloud_runtime::Error::Invalid("Cloud has no worker"))?;
    // One command reads at most MAX_SESSIONS sessions, so larger clouds read in batches.
    let mut statuses = Vec::with_capacity(ids.len());
    for batch in ids.chunks(session_status::MAX_SESSIONS) {
        statuses.extend(session_status::read(
            worker,
            &settings,
            &directory,
            batch,
            &cloud_runtime::Cancellation::default(),
        )?);
    }
    Ok(every_local(locals, &tmux, statuses))
}

/// A status for each parked panel in `locals`, from the `statuses` of the sessions in
/// `tmux`. A panel without a recorded session, or whose session the worker did not
/// report, is `Missing`: an incomplete read never shows a cloud as idle.
pub(super) fn every_local(
    locals: &[String],
    tmux: &HashMap<String, String>,
    statuses: Vec<SessionStatus>,
) -> Vec<(String, SessionStatus)> {
    let mut found: HashMap<String, SessionStatus> = statuses
        .into_iter()
        .filter_map(|status| Some((tmux.get(&status.id)?.clone(), status)))
        .collect();
    locals
        .iter()
        .map(|local| {
            let status = found.remove(local).unwrap_or_else(|| SessionStatus {
                id: tmux
                    .iter()
                    .find(|(_, panel)| *panel == local)
                    .map(|(session, _)| session.clone())
                    .unwrap_or_default(),
                activity: SessionActivity::Missing,
                quiet_for: None,
                lines: Vec::new(),
            });
            (local.clone(), status)
        })
        .collect()
}

/// The agent status that a parked panel shows for its last session status.
fn agent_status(status: Option<&SessionStatus>) -> AgentStatus {
    if status.is_some_and(|status| status.activity == SessionActivity::Working) {
        AgentStatus::Working
    } else {
        AgentStatus::Idle
    }
}

#[cfg(all(test, unix))]
mod tests;

impl HorizonApp {
    /// Starts a status read of the parked terminals of cloud `index` when one is
    /// due, and applies a finished one.
    pub(super) fn read_parked_status(&mut self, index: usize, now: Instant) {
        let issue = self.cloud_prototype.groups.0[index].issue;
        let Some(parking) = self
            .cloud_prototype
            .production
            .runtimes
            .get(&issue)
            .map(|runtime| &runtime.parking)
        else {
            return;
        };
        let due =
            parking.tracker.is_some_and(|tracker| tracker.is_parked()) && parking.next_read.is_none_or(|at| now >= at);
        if parking.reader.is_none() && !due {
            return;
        }
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
            let finished = match reader.try_recv() {
                Ok(result) => {
                    runtime.parking.reader = None;
                    runtime.parking.next_read = Some(now + STATUS_INTERVAL);
                    match result {
                        Ok(statuses) => {
                            runtime.parking.error = None;
                            runtime.parking.statuses = statuses.into_iter().collect();
                            runtime.parking.statuses_since = runtime.parking.read_started;
                        }
                        Err(error) => {
                            // An old status is not shown as current.
                            runtime.parking.error = Some(error.to_string());
                            runtime.parking.statuses.clear();
                            runtime.parking.statuses_since = None;
                        }
                    }
                    let reported: Vec<(PanelId, AgentStatus)> = parked
                        .iter()
                        .map(|(local, id)| (*id, agent_status(runtime.parking.statuses.get(local))))
                        .collect();
                    for (id, status) in reported {
                        if let Some(panel) = self.board.panel_mut(id) {
                            panel.set_parked_agent_status(status);
                        }
                    }
                    true
                }
                Err(TryRecvError::Empty) => false,
                Err(TryRecvError::Disconnected) => {
                    runtime.parking.reader = None;
                    false
                }
            };
            if finished {
                self.record_cloud_parking(index);
            }
            return;
        }
        if parked.is_empty() || runtime.parking.next_read.is_some_and(|at| now < at) {
            return;
        }
        let locals: Vec<String> = parked.into_keys().collect();
        let (Some(deployment), Some(root), Some(launch)) = (
            runtime.state.clone().filter(|state| state.worker.is_some()),
            root,
            launch,
        ) else {
            return;
        };
        runtime.parking.next_read = Some(now + STATUS_INTERVAL);
        runtime.parking.read_started = Some(now);
        let (tx, rx) = channel();
        runtime.parking.reader = Some(rx);
        let repaint = runtime.repaint_context.clone();
        std::thread::spawn(move || {
            let result = read_statuses(&deployment, &root, &launch.id, &locals);
            if tx.send(result).is_ok()
                && let Some(context) = repaint
            {
                context.request_repaint();
            }
        });
    }
}

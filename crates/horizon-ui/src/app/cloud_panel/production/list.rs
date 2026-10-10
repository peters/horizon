//! The facts of each cloud for the cloud list in the sidebar: the condition of its
//! card, what its parked sessions or attached terminals last showed, and what its
//! worker bills each hour.
use super::{HorizonApp, cards, park};
use horizon_core::{
    AgentStatus, CloudWait, Panel,
    cloud_list::{self, CloudFacts, Condition},
    cloud_panel::CloudGroup,
    cloud_runtime::session_status::SessionActivity,
};
use std::{collections::HashMap, time::SystemTime};

impl HorizonApp {
    /// The facts of each cloud on the board, by the local id of its workspace. A
    /// group without a remote launch runs on this PC, so it gives no facts.
    pub(in crate::app) fn cloud_list_facts(&self, now: SystemTime) -> HashMap<String, Vec<CloudFacts>> {
        let mut facts: HashMap<String, Vec<CloudFacts>> = HashMap::new();
        for group in self
            .cloud_prototype
            .groups
            .0
            .iter()
            .filter(|group| group.remote.is_some())
        {
            let cloud = self.cloud_facts(group, now);
            facts.entry(group.workspace.clone()).or_default().push(cloud);
        }
        facts
    }

    pub(super) fn cloud_facts(&self, group: &CloudGroup, now: SystemTime) -> CloudFacts {
        let members: Vec<&Panel> = group
            .panels
            .iter()
            .filter_map(|local| self.board.panel(self.board.panel_id_by_local_id(local)?))
            .collect();
        let primary = cloud_list::primary_panel(members.iter().copied(), self.board.focused);
        let attached_working = members.iter().any(|panel| panel.agent_status() == AgentStatus::Working);
        let Some(runtime) = self.cloud_prototype.production.runtimes.get(&group.issue) else {
            return CloudFacts {
                id: group.issue,
                name: group.title.clone(),
                condition: Condition::Idle,
                working: attached_working,
                line: "Not deployed".to_owned(),
                hourly_rate: None,
                stoppable: false,
                known: true,
            };
        };
        let (mut condition, mut line, stoppable) = cards::list::condition(group, runtime, &self.board, now);
        let mut working = attached_working;
        let mut known = true;
        // A parked cloud stays parked while its connection is down: its terminals keep
        // their snapshots, and the worker's last status is still the best line.
        if matches!(condition, Condition::Ready | Condition::Idle) && runtime.parking.is_parked() {
            let statuses = runtime.parking.statuses();
            working = statuses
                .values()
                .any(|status| status.activity == SessionActivity::Working);
            // Right after the park, after a failed read, or for a terminal that parked
            // since the last read, nothing shows that no agent works.
            known = !runtime.parking.read_failed()
                && members
                    .iter()
                    .filter(|panel| panel.cloud_wait() == Some(CloudWait::Parked))
                    .all(|panel| statuses.contains_key(&panel.local_id));
            let ended = members
                .iter()
                .filter_map(|panel| statuses.get(&panel.local_id))
                .find(|status| matches!(status.activity, SessionActivity::Exited(_) | SessionActivity::Missing));
            if let Some(ended) = ended {
                condition = Condition::Attention;
                line = park::activity_text(ended.activity);
            } else {
                condition = Condition::Parked;
                line = primary
                    .and_then(|panel| statuses.get(&panel.local_id))
                    .and_then(|status| cloud_list::status_line(status.lines.iter().map(String::as_str)))
                    .unwrap_or_else(|| "Parked".to_owned());
            }
        } else if condition == Condition::Ready
            && let Some(text) = primary.and_then(cloud_list::panel_line)
        {
            line = text;
        }
        // An agent that waits for GitHub access waits for the user.
        if condition > Condition::Attention
            && let Some(request) = runtime.github_requests.list.first()
        {
            condition = Condition::Attention;
            line = format!("GitHub {} access requested for {}", request.access, request.repository);
        }
        CloudFacts {
            id: group.issue,
            name: group.title.clone(),
            condition,
            working,
            line,
            hourly_rate: cards::list::hourly_rate(runtime),
            stoppable,
            known,
        }
    }
}

#[cfg(all(test, unix))]
mod tests;

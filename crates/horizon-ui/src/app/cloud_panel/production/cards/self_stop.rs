//! Why a cloud's worker last stopped itself, in the words of the agent that asked.
use horizon_core::cloud_runtime::{SelfStop, state::Deployment};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(super) fn show(ui: &mut egui::Ui, state: Option<&Deployment>) {
    if let Some(stop) = state.and_then(|state| state.last_self_stop.as_ref()) {
        ui.small(describe(stop, SystemTime::now()));
    }
}

fn describe(stop: &SelfStop, now: SystemTime) -> String {
    let age = UNIX_EPOCH
        .checked_add(Duration::from_millis(stop.at))
        .and_then(|at| now.duration_since(at).ok());
    let who = stop.agent.as_deref().unwrap_or("an agent");
    format!(
        "Stopped by {who} {}: {}",
        age.map_or_else(|| "just now".into(), ago),
        stop.reason
    )
}

fn ago(age: Duration) -> String {
    match age.as_secs() / 60 {
        0 => "just now".to_owned(),
        minutes @ 1..60 => format!("{minutes} min ago"),
        minutes @ 60..2880 => format!("{} h ago", minutes / 60),
        minutes => format!("{} days ago", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_says_when_and_why_an_agent_stopped_the_worker() {
        let mut stop = SelfStop {
            at: 1_790_000_000_000,
            reason: "PR 12 merged".into(),
            agent: None,
            session: None,
        };
        let at = UNIX_EPOCH + Duration::from_millis(stop.at);
        let after = |seconds| at + Duration::from_secs(seconds);
        assert_eq!(describe(&stop, after(20)), "Stopped by an agent just now: PR 12 merged");
        assert_eq!(
            describe(&stop, after(600)),
            "Stopped by an agent 10 min ago: PR 12 merged"
        );
        assert_eq!(
            describe(&stop, after(3 * 3600)),
            "Stopped by an agent 3 h ago: PR 12 merged"
        );
        assert_eq!(
            describe(&stop, after(5 * 86_400)),
            "Stopped by an agent 5 days ago: PR 12 merged"
        );
        // A worker clock ahead of this one reads as just now.
        assert_eq!(
            describe(&stop, at - Duration::from_secs(60)),
            "Stopped by an agent just now: PR 12 merged"
        );
        stop.agent = Some("claude".into());
        assert_eq!(
            describe(&stop, after(600)),
            "Stopped by claude 10 min ago: PR 12 merged"
        );
    }
}

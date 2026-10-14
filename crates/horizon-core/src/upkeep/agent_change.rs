//! What an agent changed through `cloud_upkeep`, as the board shows it under its title.
//! Horizon writes the text; nothing in it comes from the worker.
use time::OffsetDateTime;

/// How many changes a board shows.
pub const SHOWN: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentChangeKind {
    Paused,
    Resumed,
    RunRequested,
    /// The repositories removed from the Upkeep, as its settings spell them.
    Removed(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentChange {
    pub at: OffsetDateTime,
    pub kind: AgentChangeKind,
    /// The line the board shows, made once rather than on every frame.
    text: String,
}

impl AgentChange {
    #[must_use]
    pub fn now(kind: AgentChangeKind) -> Self {
        Self::at(OffsetDateTime::now_utc(), kind)
    }

    #[must_use]
    pub fn at(at: OffsetDateTime, kind: AgentChangeKind) -> Self {
        let text = text(at, &kind);
        Self { at, kind, text }
    }

    /// "An agent paused the Upkeep, 19:32 UTC"
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

fn text(at: OffsetDateTime, kind: &AgentChangeKind) -> String {
    let what = match kind {
        AgentChangeKind::Paused => "paused the Upkeep".to_string(),
        AgentChangeKind::Resumed => "resumed the Upkeep".to_string(),
        AgentChangeKind::RunRequested => "asked for a run now".to_string(),
        // Every name: an Upkeep keeps at most 50, and the board shows the full line on hover.
        AgentChangeKind::Removed(names) if names.is_empty() => "removed no repository".to_string(),
        AgentChangeKind::Removed(names) => format!("removed {}", names.join(", ")),
    };
    format!("An agent {what}, {}", super::clock(at))
}

/// Puts `change` first. It takes the place of an earlier change of the same kind, and a
/// removal also names the repositories of the earlier removals, so the [`SHOWN`] lines
/// always hold every kind of change: many runs asked for never hide a removal.
pub fn record(changes: &mut Vec<AgentChange>, mut change: AgentChange) {
    let same = |held: &AgentChange| std::mem::discriminant(&held.kind) == std::mem::discriminant(&change.kind);
    if let Some(at) = changes.iter().position(same) {
        let earlier = changes.remove(at);
        if let (AgentChangeKind::Removed(names), AgentChangeKind::Removed(newer)) = (earlier.kind, &change.kind) {
            let mut all = newer.clone();
            all.extend(names.into_iter().filter(|name| !newer.contains(name)));
            change = AgentChange::at(change.at, AgentChangeKind::Removed(all));
        }
    }
    changes.insert(0, change);
    changes.truncate(SHOWN);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_keeps_its_newest_line_and_a_removal_is_never_hidden() {
        let at = OffsetDateTime::from_unix_timestamp(70_320).unwrap();
        let mut changes = Vec::new();
        let removed = |names: &[&str]| AgentChangeKind::Removed(names.iter().map(|name| (*name).to_owned()).collect());
        let mut kinds = vec![removed(&["acme/web"])];
        kinds.extend(std::iter::repeat_n(AgentChangeKind::RunRequested, 6));
        kinds.extend([
            AgentChangeKind::Paused,
            AgentChangeKind::Resumed,
            removed(&["acme/api", "acme/web"]),
            removed(&["acme/app"]),
            AgentChangeKind::Paused,
        ]);
        for kind in kinds {
            record(&mut changes, AgentChange::at(at, kind));
        }
        let texts: Vec<&str> = changes.iter().map(AgentChange::text).collect();
        assert_eq!(
            texts,
            [
                "An agent paused the Upkeep, 19:32 UTC",
                "An agent removed acme/app, acme/api, acme/web, 19:32 UTC",
                "An agent resumed the Upkeep, 19:32 UTC",
                "An agent asked for a run now, 19:32 UTC",
            ]
        );
        assert_eq!(changes[1].kind, removed(&["acme/app", "acme/api", "acme/web"]));
        record(&mut changes, AgentChange::at(at, removed(&["acme/one"])));
        assert_eq!(
            changes[0].text(),
            "An agent removed acme/one, acme/app, acme/api, acme/web, 19:32 UTC"
        );
    }
}

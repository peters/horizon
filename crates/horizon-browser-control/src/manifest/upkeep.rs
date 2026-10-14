//! Agent requests for Upkeep, carried by the provider usage queue. The host answers only
//! for the Upkeep in the caller's workspace and authorizes every field against its records.
//! The limits match the settings that Horizon accepts for an Upkeep.
use serde::{Deserialize, Serialize};

/// The most repositories one Upkeep keeps moving.
const MAX_REPOSITORIES: usize = 50;
/// The bots whose pull requests an Upkeep can follow: the ones New Upkeep offers.
const AUTHORS: [&str; 2] = ["app/dependabot", "app/renovate"];
/// The longest instructions a request carries, in bytes.
const MAX_INSTRUCTIONS: usize = 8 * 1024;

/// What an agent asks of Upkeep. `status` and `discover` only read.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpkeepOperation {
    /// The Upkeep in the caller's workspace: repositories, schedule, runs and pull requests.
    #[default]
    Status,
    /// The repositories the GitHub app reaches, with their open pull requests from the bots.
    Discover,
    /// Shows New Upkeep in the caller's workspace with these settings, for the person to start.
    Create,
    /// Shows the board's repository list with these repositories added, for the person to save.
    Add,
    /// Removes repositories from the Upkeep.
    Remove,
    /// Starts a run now instead of at the next scheduled time.
    RunNow,
    /// Stops the scheduled runs; a run in progress finishes.
    Pause,
    /// Starts the scheduled runs again.
    Resume,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UpkeepRequest {
    pub operation: UpkeepOperation,
    /// GitHub `owner/name` repositories for `create`, `add` and `remove`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repositories: Vec<String>,
    /// The bots, `app/dependabot` or `app/renovate`, for `create` and `discover`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// 60, 240 or 1440, for `create`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_minutes: Option<u32>,
    /// The instructions every run starts from, for `create`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// `on_demand`, `partial` or `full`, for `create`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkouts: Option<String>,
}

impl UpkeepRequest {
    /// Whether the fields this operation needs are present and well formed. The host still
    /// authorizes the request against the Upkeep of the caller's workspace.
    #[must_use]
    pub fn valid(&self) -> bool {
        let repositories =
            self.repositories.len() <= MAX_REPOSITORIES && self.repositories.iter().all(|name| repository(name));
        let authors = self.authors.iter().all(|author| AUTHORS.contains(&author.as_str()))
            && !self
                .authors
                .iter()
                .enumerate()
                .any(|(at, author)| self.authors[..at].contains(author));
        let every = self
            .every_minutes
            .is_none_or(|minutes| matches!(minutes, 60 | 240 | 1440));
        let instructions = self.instructions.as_deref().is_none_or(|text| {
            !text.trim().is_empty()
                && text.len() <= MAX_INSTRUCTIONS
                && !text
                    .chars()
                    .any(|c| (c.is_control() && c != '\n' && c != '\t') || invisible(c))
        });
        let checkouts = self
            .checkouts
            .as_deref()
            .is_none_or(|checkouts| matches!(checkouts, "on_demand" | "partial" | "full"));
        if !repositories || !authors || !every || !instructions || !checkouts {
            return false;
        }
        let create_only = self.every_minutes.is_none() && self.instructions.is_none() && self.checkouts.is_none();
        match self.operation {
            UpkeepOperation::Status | UpkeepOperation::RunNow | UpkeepOperation::Pause | UpkeepOperation::Resume => {
                self.repositories.is_empty() && self.authors.is_empty() && create_only
            }
            UpkeepOperation::Discover => self.repositories.is_empty() && create_only,
            UpkeepOperation::Create => !self.repositories.is_empty(),
            UpkeepOperation::Add | UpkeepOperation::Remove => {
                !self.repositories.is_empty() && self.authors.is_empty() && create_only
            }
        }
    }
}

/// A character that changes no glyph on the screen, such as a zero-width space, a
/// direction mark or a tag character: text made of these can say what a person never sees.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF0}'..='\u{FFFB}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}

/// A GitHub login: letters, digits and inner hyphens, at most 39 characters.
fn login(name: &str) -> bool {
    (1..=39).contains(&name.len())
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// `owner/name` as GitHub spells it.
fn repository(name: &str) -> bool {
    let mut parts = name.split('/');
    let (Some(owner), Some(repository), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    login(owner)
        && (1..=100).contains(&repository.len())
        && repository != "."
        && repository != ".."
        && repository
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(value: serde_json::Value) -> Result<UpkeepRequest, serde_json::Error> {
        serde_json::from_value(value)
    }

    fn valid(value: serde_json::Value) -> bool {
        request(value).unwrap().valid()
    }

    #[test]
    fn each_operation_takes_only_its_own_fields() {
        use serde_json::json;
        assert!(valid(json!({"operation": "status"})));
        assert!(valid(
            json!({"operation": "discover", "authors": ["app/dependabot", "app/renovate"]})
        ));
        assert!(valid(
            json!({"operation": "create", "repositories": ["acme/web"], "every_minutes": 60, "checkouts": "full"})
        ));
        assert!(valid(
            json!({"operation": "add", "repositories": ["acme/web", "acme/api"]})
        ));
        assert!(!valid(json!({"operation": "add"})));
        assert!(!valid(json!({"operation": "create"})));
        assert!(!valid(
            json!({"operation": "add", "repositories": ["acme/web"], "every_minutes": 60})
        ));
        assert!(!valid(
            json!({"operation": "remove", "repositories": ["acme/web"], "authors": ["app/x"]})
        ));
        assert!(!valid(json!({"operation": "run_now", "repositories": ["acme/web"]})));
        assert!(!valid(json!({"operation": "pause", "authors": ["app/dependabot"]})));
        assert!(!valid(
            json!({"operation": "discover", "authors": ["app/renovate", "app/renovate"]})
        ));
        assert!(!valid(
            json!({"operation": "create", "repositories": ["acme/web"], "every_minutes": 5})
        ));
        assert!(!valid(
            json!({"operation": "create", "repositories": ["acme/web"], "checkouts": "deep"})
        ));
        assert!(!valid(
            json!({"operation": "create", "repositories": ["acme/web"], "instructions": " "})
        ));
        assert!(!valid(
            json!({"operation": "create", "repositories": ["acme/web"], "instructions": "fix\u{1b}[2J"})
        ));
        for hidden in ["merge\u{200B}all", "a\u{202E}b", "x\u{E0041}", "\u{FEFF}fix"] {
            assert!(
                !valid(json!({"operation": "create", "repositories": ["acme/web"], "instructions": hidden})),
                "{hidden:?}"
            );
        }
        assert!(valid(
            json!({"operation": "create", "repositories": ["acme/web"], "instructions": "Fix CI.\n\tThen merge; café ✓"})
        ));
        let long = "x".repeat(MAX_INSTRUCTIONS + 1);
        assert!(!valid(
            json!({"operation": "create", "repositories": ["acme/web"], "instructions": long})
        ));
        let many: Vec<String> = (0..=MAX_REPOSITORIES).map(|n| format!("acme/r{n}")).collect();
        assert!(!valid(json!({"operation": "add", "repositories": many})));
    }

    #[test]
    fn names_cannot_leave_github_or_start_an_option() {
        use serde_json::json;
        for bad in [
            "acme",
            "acme/../x",
            "acme/web/extra",
            "acme/ web",
            "../web",
            "acme/..",
            "-acme/web",
            "acme/",
        ] {
            assert!(!valid(json!({"operation": "add", "repositories": [bad]})), "{bad}");
        }
        for bad in [
            "-dependabot",
            "app/",
            "app/x y",
            "renovate[bot]",
            "app/dependabot[bot]",
            "app/github-actions",
        ] {
            assert!(!valid(json!({"operation": "discover", "authors": [bad]})), "{bad}");
        }
        assert!(request(json!({"operation": "status", "workspace": "w"})).is_err());
        assert!(request(json!({"operation": "create", "repositories": ["a/b"], "agent": "codex"})).is_err());
        assert!(request(json!({"operation": "delete"})).is_err());
    }
}

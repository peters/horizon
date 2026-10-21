//! The rules of a profile host that need no browser: which create it takes,
//! the ID of a new page, and which page is idle.

use std::path::Path;
use std::time::{Duration, SystemTime};

use horizon_browser_control::manifest::{self, BrowserCreateRequest, actor_is_workspace_scoped};

use super::config::ProfileConfig;
use super::host::{MAX_ACTOR_PAGES, MAX_PAGES};

/// Why a create is refused, with the code that the MCP client sees.
#[derive(Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

pub fn refuse(code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal {
        code,
        message: message.into(),
    }
}

/// The profile that a new page opens in, or the refusal of the create.
/// `source` is the profile and the actor of the page that a duplicate names.
/// `running` counts every page of the host, and `mine` the open pages of the
/// actor of the request.
///
/// # Errors
/// The request names no target, a remote target, an unknown profile, a
/// duplicate of another actor's page, the actor already runs its pages, or
/// the host already runs its pages. The last page goes only to an actor with
/// no page, so one or more busy actors never lock out a new client.
pub fn admit<'a>(
    config: &'a ProfileConfig,
    request: &BrowserCreateRequest,
    source: Option<(&'a str, &str)>,
    running: usize,
    mine: usize,
) -> Result<&'a str, Refusal> {
    if !actor_is_workspace_scoped(&request.actor) {
        return Err(refuse(
            "browser_start_failed",
            "a profile host serves only agent identities",
        ));
    }
    if manifest::now_millis() >= request.deadline_at_millis {
        return Err(refuse("browser_start_failed", "Browser creation deadline expired"));
    }
    let name = if let Some(duplicate) = request.duplicate_from.as_deref() {
        match source {
            Some((profile, actor)) if actor == request.actor => profile,
            _ => {
                return Err(refuse(
                    "not_found",
                    format!("{duplicate} is not a page of this agent on this profile host"),
                ));
            }
        }
    } else {
        let Some(target) = request.target.as_deref() else {
            return Err(refuse(
                "target_required",
                "this profile host opens pages only in its profiles; name a profile as the target",
            ));
        };
        if horizon_browser::provider_catalog::target_provider(target).is_some() {
            return Err(refuse(
                "target_invalid",
                "a profile host serves no remote target; a standalone host with a host file of remote targets (mcp --standalone --config) does",
            ));
        }
        let Some((name, _)) = config.profiles.get_key_value(target) else {
            return Err(refuse(
                "target_unknown",
                format!("profile `{target}` is not in the host file"),
            ));
        };
        name.as_str()
    };
    if request.orientation.is_some() || request.backend.is_some() {
        return Err(refuse(
            "unsupported_backend",
            "a profile selects its engine; a page takes no backend and no orientation",
        ));
    }
    if mine >= MAX_ACTOR_PAGES {
        return Err(refuse(
            "page_limit_reached",
            format!("this agent already runs its maximum of {MAX_ACTOR_PAGES} pages on this profile host"),
        ));
    }
    if running >= MAX_PAGES || (mine > 0 && running + 1 >= MAX_PAGES) {
        return Err(refuse(
            "page_limit_reached",
            format!(
                "this profile host already runs its maximum of {MAX_PAGES} pages, the last one for a client with no page"
            ),
        ));
    }
    Ok(name)
}

/// What a close request does to a page.
#[derive(Debug, PartialEq, Eq)]
pub enum Close {
    /// Stop the open page.
    Stop,
    /// The page already stops: answer the close when it stopped.
    Wait,
    /// No page of the actor of the request has this ID.
    NotFound,
}

/// What a close of a page does. `open` is the actor of the open page with
/// the ID of the request, and `stopping` the actor of the stopping page with
/// that ID. Only the actor of a page closes it, also while it stops.
#[must_use]
pub fn close_action(open: Option<&str>, stopping: Option<&str>, actor: &str) -> Close {
    match (open, stopping) {
        (Some(owner), _) if owner == actor => Close::Stop,
        (None, Some(owner)) if owner == actor => Close::Wait,
        _ => Close::NotFound,
    }
}

/// A new page ID: `profile-<name>-` and 12 hex digits.
#[must_use]
pub fn page_id(profile: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("profile-{profile}-{}", &hex[..12])
}

/// Whether a page whose last action was at `last` is idle at `now`.
#[must_use]
pub fn idle(last: SystemTime, now: SystemTime, limit: Duration) -> bool {
    now.duration_since(last).is_ok_and(|age| age >= limit)
}

/// The IDs of the ready pages under `root` with no action within `limit`:
/// each ID with the time that its page opened, or of its last screenshot.
#[must_use]
pub fn idle_pages<'a>(
    root: &Path,
    ready: impl Iterator<Item = (&'a str, SystemTime)>,
    now: SystemTime,
    limit: Duration,
) -> Vec<String> {
    ready
        .filter(|(id, opened)| idle(last_action(root, id, *opened), now, limit))
        .map(|(id, _)| id.to_owned())
        .collect()
}

/// The last action of a page that its audit journal does not show: its
/// opening, or its last screenshot when that is later.
#[must_use]
pub fn host_action(opened: SystemTime, screenshot: Option<SystemTime>) -> SystemTime {
    screenshot.map_or(opened, |at| at.max(opened))
}

/// The time of the last line of the audit journal of a page, or the time
/// that the host gave, if that is later.
fn last_action(root: &Path, id: &str, opened: SystemTime) -> SystemTime {
    std::fs::metadata(manifest::audit_path_for_root(root, id))
        .and_then(|metadata| metadata.modified())
        .map_or(opened, |modified| modified.max(opened))
}

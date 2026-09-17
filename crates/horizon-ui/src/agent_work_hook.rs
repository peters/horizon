use std::io::{self, Read};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use horizon_core::PanelKind;
use horizon_core::agent_work::{HookInput, WORK_KIND_ENV, WORK_OWNER_ENV, WORK_PANEL_ENV, WORK_ROOT_ENV, WorkStore};

const MAX_HOOK_BYTES: u64 = 1024 * 1024;

/// Runs before tracing, plugin installation, or GUI startup. The command is
/// inert without the panel-specific environment set by an opted-in launch.
pub(crate) fn run_if_requested() -> bool {
    if std::env::args().nth(1).as_deref() != Some("--agent-work-hook") {
        return false;
    }
    let Some(context) = HookContext::from_environment() else {
        return true;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let now = i64::try_from(now).unwrap_or(i64::MAX);
    if let Err(error) = context.process(io::stdin().lock(), now) {
        if context.store.invalidate(&context.panel, &context.owner).is_err() {
            eprintln!("agent work evidence could not be invalidated; continuation requires manual review");
            // Exit 2 can make a Stop hook continue the agent. Recorder errors
            // must report failure without changing provider turn behavior.
            std::process::exit(3);
        }
        eprintln!("agent work hook could not save evidence ({:?})", error.kind());
        std::process::exit(1);
    }
    true
}

struct HookContext {
    store: WorkStore,
    panel: String,
    owner: String,
    kind: PanelKind,
}

impl HookContext {
    fn from_environment() -> Option<Self> {
        let kind = match std::env::var(WORK_KIND_ENV).ok()?.as_str() {
            "claude" => PanelKind::Claude,
            "codex" => PanelKind::Codex,
            _ => return None,
        };
        let root = PathBuf::from(std::env::var_os(WORK_ROOT_ENV)?);
        if !root.is_absolute() {
            return None;
        }
        Some(Self {
            store: WorkStore::new(&root),
            panel: std::env::var(WORK_PANEL_ENV).ok()?,
            owner: std::env::var(WORK_OWNER_ENV).ok()?,
            kind,
        })
    }

    fn process(&self, input: impl Read, now: i64) -> io::Result<()> {
        let mut bytes = Vec::new();
        input.take(MAX_HOOK_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_HOOK_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "hook input exceeds limit"));
        }
        let input: HookInput = serde_json::from_slice(&bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid lifecycle input"))?;
        self.store.apply_hook(&self.panel, self.kind, &self.owner, &input, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_and_oversized_inputs_do_not_create_evidence() {
        let temp = tempfile::tempdir().expect("fixture");
        let context = HookContext {
            store: WorkStore::new(temp.path()),
            panel: "panel".into(),
            owner: "owner".into(),
            kind: PanelKind::Claude,
        };
        assert!(context.process(b"not JSON".as_slice(), 1).is_err());
        assert!(context.process(io::repeat(b' ').take(MAX_HOOK_BYTES + 1), 1).is_err());
        assert!(context.store.read("panel").expect("read").is_none());
    }
}

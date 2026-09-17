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
    let result = context.process(io::stdin().lock(), now);
    if let Ok(Some(additional_context)) = &result {
        println!(
            "{}",
            serde_json::json!({"hookSpecificOutput": {"hookEventName":"SessionStart", "additionalContext":additional_context}})
        );
    }
    if let Err(error) = result {
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

    fn process(&self, input: impl Read, now: i64) -> io::Result<Option<String>> {
        let token = self.store.begin_hook(&self.panel, &self.owner)?;
        let mut bytes = Vec::new();
        input.take(MAX_HOOK_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_HOOK_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "hook input exceeds limit"));
        }
        let input: HookInput = serde_json::from_slice(&bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid lifecycle input"))?;
        self.store
            .apply_hook(&self.panel, self.kind, &self.owner, &input, now)?;
        self.store.finish_hook(&self.panel, &self.owner, &token)?;
        if input.event.hook_event_name == "SessionStart"
            && input.event.source.as_deref() == Some("resume")
            && let Some(record) = self.store.read(&self.panel)?
            && record.owner_token == self.owner
            && record.ledger.session_id == input.event.session_id
            && let Some(handoff) = &record.handoff
            && handoff.session_id == input.event.session_id
            && handoff.generation.saturating_add(2) == record.ledger.generation
            && handoff.cwd == input.cwd.to_string_lossy()
        {
            return Ok(Some(horizon_core::agent_work::resume_brief(&record, now)));
        }
        Ok(None)
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
        context
            .store
            .register_owner("panel", PanelKind::Claude, "owner", Some("session"), temp.path())
            .expect("register");
        let malformed = context.process(b"not JSON".as_slice(), 1).expect_err("malformed input");
        assert_eq!(malformed.to_string(), "invalid lifecycle input");
        let oversized = context
            .process(io::repeat(b' ').take(MAX_HOOK_BYTES + 1), 1)
            .expect_err("oversized input");
        assert_eq!(oversized.to_string(), "hook input exceeds limit");
        assert!(context.store.read("panel").is_err());
    }
    #[test]
    fn resume_context_is_metadata_only_and_only_for_the_first_matching_launch() {
        use horizon_core::agent_work::{SuspendRecord, TranscriptSnapshot, TurnState};
        let temp = tempfile::tempdir().expect("fixture");
        let context = HookContext {
            store: WorkStore::new(temp.path()),
            panel: "panel".into(),
            owner: "old".into(),
            kind: PanelKind::Claude,
        };
        context
            .store
            .register_owner("panel", context.kind, "old", Some("session"), temp.path())
            .expect("owner");
        let path = temp.path().join("transcript.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"message\":{\"content\":\"private prompt\"}}\n",
        )
        .expect("transcript");
        let event = |name: &str, source: &str| {
            serde_json::to_vec(&serde_json::json!({"session_id":"session","prompt_id":"prompt","hook_event_name":name,"source":source,"cwd":temp.path(),"transcript_path":path})).expect("event")
        };
        context
            .process(event("UserPromptSubmit", "").as_slice(), 10)
            .expect("prompt");
        context
            .store
            .save_handoff(
                "old",
                SuspendRecord {
                    kind: context.kind,
                    before_cancel: Some(TranscriptSnapshot {
                        bytes: 1,
                        tail_sha256: [0; 32],
                        state: TurnState::Working,
                    }),
                    panel_local_id: context.panel.clone(),
                    session_id: "session".into(),
                    prompt_id: "prompt".into(),
                    generation: 1,
                    suspended_at_millis: 11,
                    cwd: temp.path().to_str().expect("cwd").into(),
                    repo_fingerprint: None,
                    final_transcript: None,
                    cancelled_by_horizon: true,
                },
            )
            .expect("handoff");
        context
            .store
            .register_owner("panel", context.kind, "new", Some("session"), temp.path())
            .expect("new launch");
        let context = HookContext {
            owner: "new".into(),
            ..context
        };
        let brief = context
            .process(event("SessionStart", "resume").as_slice(), 20)
            .expect("hook")
            .expect("context");
        assert!(brief.contains("Unix time 11 ms"));
        assert!(!brief.contains("private prompt"));
        assert!(
            context
                .process(event("SessionStart", "resume").as_slice(), 21)
                .expect("repeat")
                .is_none()
        );
        assert!(
            context
                .process(event("UserPromptSubmit", "").as_slice(), 22)
                .expect("new prompt")
                .is_none()
        );
    }
}

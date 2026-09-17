use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::PanelKind;
use crate::horizon_home::HorizonHome;
use crate::panel::current_unix_millis;

use super::{
    ResumePolicy, SuspendRecord, TranscriptSnapshot, TurnState, WORK_EXECUTABLE_ENV, WORK_KIND_ENV, WORK_OWNER_ENV,
    WORK_PANEL_ENV, WORK_ROOT_ENV, WorkStore,
};

pub(crate) struct WorkLaunch<'a> {
    pub panel: &'a str,
    pub kind: PanelKind,
    pub policy: &'a ResumePolicy,
    pub cwd: Option<&'a Path>,
    pub session_id: Option<&'a str>,
    pub default_command: bool,
}

pub(crate) struct WorkOwner {
    store: WorkStore,
    panel: String,
    token: String,
    interrupted: AtomicBool,
}

impl WorkLaunch<'_> {
    pub(crate) fn attach_launch(
        &self,
        program: &str,
        unambiguous: bool,
        args: &mut [String],
        env: &mut HashMap<String, String>,
    ) -> Option<Arc<WorkOwner>> {
        let owned = unambiguous.then(|| self.owned_command(program, args, env)).flatten();
        self.attach(owned.as_deref(), args, env)
    }

    pub(crate) fn owned_command(
        &self,
        program: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Option<String> {
        if !self.policy.enabled || !self.default_command || !self.kind.is_agent() {
            return None;
        }
        super::command::owned_command(program, args, self.cwd, env)
    }

    pub(crate) fn attach(
        &self,
        owned_command: Option<&str>,
        args: &mut [String],
        env: &mut HashMap<String, String>,
    ) -> Option<Arc<WorkOwner>> {
        let owned_command = owned_command?;
        // The empirical teardown/seeded-resume proof currently covers Linux.
        // Other providers and platforms retain the explicit confirmation path.
        if !cfg!(target_os = "linux") || !self.policy.enabled || self.kind != PanelKind::Claude || !self.default_command
        {
            return None;
        }
        if args.len() != 2 || args[0] != "-ic" {
            return None;
        }
        let cwd = self
            .cwd
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())?
            .canonicalize()
            .ok()?;
        let home = HorizonHome::resolve();
        let plugin = home.agent_work_plugin_dir_for_host(crate::browser::manifest::host_instance());
        if !plugin.join("hooks/hooks.json").is_file() {
            return None;
        }
        let executable = std::env::current_exe().ok()?;
        let executable = executable.to_str()?;
        let root = home.root().to_str()?;
        let plugin = plugin.to_str()?;
        let token = uuid::Uuid::new_v4().to_string();
        let store = WorkStore::new(home.root());
        if store
            .register_owner(self.panel, self.kind, &token, self.session_id, &cwd)
            .is_err()
        {
            tracing::warn!("work continuation evidence could not be initialized");
            return None;
        }
        // The PTY must own the provider, not an intermediate shell, so joining
        // its teardown proves provider exit before the final transcript read.
        owned_command.clone_into(&mut args[1]);
        args[1].push_str(" --plugin-dir ");
        args[1].push_str(&quote_argument(plugin));
        for (key, value) in [
            (WORK_ROOT_ENV, root),
            (WORK_PANEL_ENV, self.panel),
            (WORK_OWNER_ENV, &token),
            (WORK_KIND_ENV, "claude"),
            (WORK_EXECUTABLE_ENV, executable),
        ] {
            env.insert(key.to_owned(), value.to_owned());
        }
        Some(Arc::new(WorkOwner {
            store,
            panel: self.panel.to_owned(),
            token,
            interrupted: AtomicBool::new(false),
        }))
    }
}

impl WorkOwner {
    pub(crate) fn note_input(&self, bytes: &[u8]) {
        // Veto the entire launch after user cancellation. Even a later turn
        // does not clear this until a new launch: transcript writes can lag input.
        if cancellation_input(bytes) {
            self.interrupted.store(true, Ordering::Release);
        }
    }

    pub(crate) fn prepare_suspend(&self) -> bool {
        if self.interrupted.load(Ordering::Acquire) {
            return false;
        }
        self.save_suspend().is_some()
    }

    fn save_suspend(&self) -> Option<()> {
        let record = self.store.read(&self.panel).ok()??;
        if record.owner_token != self.token
            || record.kind != PanelKind::Claude
            || record.ledger.state != TurnState::Working
        {
            return None;
        }
        let before = TranscriptSnapshot::read(&record.transcript_path, record.kind).ok()?;
        if before.state != TurnState::Working {
            return None;
        }
        let fingerprint = super::repository::fingerprint(&record.cwd);
        if self.interrupted.load(Ordering::Acquire) {
            return None;
        }
        self.store
            .save_handoff(
                &self.token,
                SuspendRecord {
                    kind: record.kind,
                    panel_local_id: self.panel.clone(),
                    session_id: record.ledger.session_id,
                    prompt_id: record.ledger.prompt_id?,
                    generation: record.ledger.generation,
                    suspended_at_millis: current_unix_millis(),
                    cwd: record.cwd.to_str()?.to_owned(),
                    repo_fingerprint: fingerprint,
                    before_cancel: Some(before),
                    final_transcript: None,
                    cancelled_by_horizon: true,
                },
            )
            .ok()
    }

    pub(crate) fn finish_suspend(&self) {
        if self.interrupted.load(Ordering::Acquire) {
            let _ = self.store.invalidate(&self.panel, &self.token);
            return;
        }
        let result = (|| {
            let record = self
                .store
                .read(&self.panel)?
                .ok_or_else(|| std::io::Error::other("missing record"))?;
            let snapshot = TranscriptSnapshot::read(&record.transcript_path, record.kind)?;
            self.store.finish_handoff(&self.panel, &self.token, snapshot)
        })();
        if self.interrupted.load(Ordering::Acquire) {
            let _ = self.store.invalidate(&self.panel, &self.token);
        }
        if result.is_err() {
            tracing::debug!("work shutdown evidence incomplete; automatic continuation remains disabled");
        }
    }
}

fn cancellation_input(bytes: &[u8]) -> bool {
    if bytes.contains(&3) || bytes.last() == Some(&27) {
        return true;
    }
    let Some(encoded) = bytes.strip_prefix(b"\x1b[").and_then(|value| value.strip_suffix(b"u")) else {
        return false;
    };
    let Ok(encoded) = std::str::from_utf8(encoded) else {
        return false;
    };
    let mut fields = encoded.split(';');
    let key = fields.next().unwrap_or_default().split(':').next().unwrap_or_default();
    if key == "27" {
        return true;
    }
    let modifiers = fields
        .next()
        .unwrap_or("1")
        .split(':')
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(1);
    matches!(key, "99" | "67") && modifiers.saturating_sub(1) & 4 != 0
}

pub(super) fn quote_argument(argument: &str) -> String {
    format!("'{}'", argument.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_interrupt_remains_a_veto_despite_later_typed_input() {
        let directory = tempfile::tempdir().expect("fixture");
        let owner = WorkOwner {
            store: WorkStore::new(directory.path()),
            panel: "panel".into(),
            token: "owner".into(),
            interrupted: AtomicBool::new(false),
        };
        owner.note_input(b"ordinary input");
        assert!(!owner.interrupted.load(Ordering::Acquire));
        owner.note_input(b"\x1b");
        owner.note_input(b"later prompt\r");
        assert!(owner.interrupted.load(Ordering::Acquire));
        assert!(!owner.prepare_suspend());
    }

    #[test]
    fn navigation_and_paste_do_not_count_as_user_interrupts() {
        for input in [b"\x1b[A".as_slice(), b"\x1b[200~text\x1b[201~", b"\x1b[99u"] {
            assert!(!cancellation_input(input));
        }
        for input in [b"\x1b".as_slice(), b"\x03", b"\x1b[27u", b"\x1b[99;5u", b"\x1b[99;5:1u"] {
            assert!(cancellation_input(input));
        }
    }

    #[test]
    fn suspend_records_the_working_tail_before_cancel_and_seals_only_after_end() {
        let directory = tempfile::tempdir().expect("fixture");
        let path = directory.path().join("transcript.jsonl");
        std::fs::write(&path, "{\"type\":\"user\",\"message\":{\"content\":\"work\"}}\n").expect("transcript");
        let store = WorkStore::new(directory.path());
        store
            .register_owner("panel", PanelKind::Claude, "owner", Some("session"), directory.path())
            .expect("owner");
        let mut input: super::super::HookInput = serde_json::from_value(serde_json::json!({
            "session_id": "session", "prompt_id": "prompt", "hook_event_name": "UserPromptSubmit",
            "cwd": directory.path(), "transcript_path": path
        }))
        .expect("input");
        store
            .apply_hook("panel", PanelKind::Claude, "owner", &input, current_unix_millis())
            .expect("working");
        let owner = WorkOwner {
            store: store.clone(),
            panel: "panel".into(),
            token: "owner".into(),
            interrupted: AtomicBool::new(false),
        };
        assert!(owner.prepare_suspend());
        let before = store
            .read("panel")
            .expect("read")
            .expect("record")
            .handoff
            .expect("handoff");
        assert_eq!(before.before_cancel.expect("before").state, TurnState::Working);
        assert!(before.final_transcript.is_none());
        owner.finish_suspend();
        assert!(
            store
                .read("panel")
                .expect("read")
                .expect("record")
                .handoff
                .expect("handoff")
                .final_transcript
                .is_none()
        );
        input.event.hook_event_name = "SessionEnd".into();
        input.event.reason = Some("other".into());
        store
            .apply_hook("panel", PanelKind::Claude, "owner", &input, current_unix_millis())
            .expect("end");
        owner.finish_suspend();
        assert!(
            store
                .read("panel")
                .expect("read")
                .expect("record")
                .handoff
                .expect("handoff")
                .final_transcript
                .is_some()
        );
        owner.note_input(b"\x03");
        owner.finish_suspend();
        assert!(store.read("panel").is_err());
    }

    #[test]
    fn plugin_path_is_always_shell_quoted() {
        assert_eq!(quote_argument("a;'b"), "'a;'\\''b'");
    }
}

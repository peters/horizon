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
    /// Whether this launch continues the work of its panel. Only such a launch
    /// may prepare a handoff and interrupt its agent at shutdown; a launch that
    /// only reports its state writes no evidence of a continuation.
    continues: bool,
}

impl WorkLaunch<'_> {
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

    /// Whether this launch reports its state through a work ledger: a launch of
    /// an agent kind that Horizon gives its own arguments to, on Linux or on
    /// macOS. A command of the preset and the policy of the panel do not take
    /// part in this: the sidebar reads the state of every such panel.
    ///
    /// The report needs no read of a process table. The launch token that
    /// Horizon registers is the owner of the panel while that panel lives: a new
    /// launch replaces it, and a closed panel removes it, so a live panel cannot
    /// inherit the state of an older process. The liveness probe of the session
    /// registry in [`super::discovery`] stays with the continuation of work.
    pub(crate) fn is_observed(&self) -> bool {
        cfg!(unix) && crate::agents::receives_horizon_arguments(self.kind)
    }

    /// Whether this launch continues the work of its panel after a restart: a
    /// Claude panel with its own default command, its continuation policy and
    /// the process reads of its system. Only this path offers a resume, and
    /// only it owns the provider process so that a teardown proves the exit
    /// before the final transcript read.
    ///
    /// The teardown and the seeded resume need the owned provider command, a
    /// process presence that signal 0 answers and a start time, and each of
    /// those answers on every Unix. The platform gate that held back macOS is
    /// gone with the fallbacks that made a Mac answer a wrong value.
    pub(crate) fn continues_work(&self) -> bool {
        cfg!(unix) && self.policy.enabled && self.kind == PanelKind::Claude && self.default_command
    }

    /// Give the agent of this panel the work ledger of its launch.
    ///
    /// A launch that continues the work of its panel needs the owned provider
    /// command, because only a joined provider proves its exit. A launch that is
    /// only observed needs nothing more than the report of its agent: the
    /// record, the folder or the file that the program loads, and the five
    /// variables. A kind whose report is not on disk attaches nothing, because
    /// the program would not start without it.
    pub(crate) fn attach(
        &self,
        owned_command: Option<&str>,
        args: &mut [String],
        env: &mut HashMap<String, String>,
    ) -> Option<Arc<WorkOwner>> {
        if !self.is_observed() || (self.continues_work() && owned_command.is_none()) {
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
        let report = self.report_argument(&home)?;
        let kind = super::work_kind_name(self.kind)?;
        let executable = std::env::current_exe().ok()?;
        let executable = executable.to_str()?;
        let root = home.root().to_str()?;
        let token = uuid::Uuid::new_v4().to_string();
        let store = WorkStore::new(home.root());
        if store
            .register_owner(self.panel, self.kind, &token, self.session_id, &cwd)
            .is_err()
        {
            tracing::warn!("the work ledger of this launch could not be initialized");
            return None;
        }
        // The PTY must own the provider, not an intermediate shell, so joining
        // its teardown proves provider exit before the final transcript read.
        if let Some(owned_command) = owned_command {
            owned_command.clone_into(&mut args[1]);
        }
        args[1].push_str(&report);
        for (key, value) in [
            (WORK_ROOT_ENV, root),
            (WORK_PANEL_ENV, self.panel),
            (WORK_OWNER_ENV, &token),
            (WORK_KIND_ENV, kind),
            (WORK_EXECUTABLE_ENV, executable),
        ] {
            env.insert(key.to_owned(), value.to_owned());
        }
        Some(Arc::new(WorkOwner {
            store,
            panel: self.panel.to_owned(),
            token,
            interrupted: AtomicBool::new(false),
            continues: self.continues_work(),
        }))
    }

    /// The argument that makes the agent of this panel report its state, with
    /// the file that the argument names, or `None` when that file is not on disk.
    ///
    /// A Claude launch loads the plugin folder of the work hooks with
    /// `--plugin-dir`; a pi launch loads the report extension with `-e`. Both are
    /// shell-quoted, so a path stays data, and both are appended to the command
    /// of the panel whatever command its preset names, as Claude's own command
    /// does today.
    fn report_argument(&self, home: &HorizonHome) -> Option<String> {
        let host = crate::browser::manifest::host_instance();
        let (flag, path, present) = match self.kind {
            PanelKind::Claude => {
                let path = home.agent_work_plugin_dir_for_host(host);
                let present = path.join("hooks/hooks.json").is_file();
                ("--plugin-dir", path, present)
            }
            PanelKind::Pi => {
                let path = home.agent_work_extension_path_for_host(host);
                let present = path.is_file();
                ("-e", path, present)
            }
            _ => return None,
        };
        let path = path.to_str()?;
        let report = format!(" {flag} {}", quote_argument(path));
        present.then_some(report)
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

    /// Whether this launch continues the work of its panel at a shutdown. A
    /// launch that only reports its state ends with its process and leaves no
    /// handoff, so the panel of a report shuts down like any other terminal.
    pub(crate) fn continues_work(&self) -> bool {
        self.continues
    }

    pub(crate) fn prepare_suspend(&self) -> bool {
        if !self.continues || self.interrupted.load(Ordering::Acquire) {
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
    use crate::agents::all_agent_kinds;

    fn gates(kind: PanelKind, enabled: bool, default_command: bool) -> (bool, bool) {
        let policy = ResumePolicy {
            enabled,
            ..ResumePolicy::default()
        };
        let launch = WorkLaunch {
            panel: "panel",
            kind,
            policy: &policy,
            cwd: None,
            session_id: Some("session"),
            default_command,
        };
        (launch.is_observed(), launch.continues_work())
    }

    #[test]
    fn observation_does_not_depend_on_the_command_or_the_policy_of_the_panel() {
        // The sidebar reads the state of a Claude panel whatever the preset
        // runs and whatever the panel's continuation policy is.
        for (enabled, default_command) in [(false, true), (true, true), (false, false), (true, false)] {
            let (observed, continues) = gates(PanelKind::Claude, enabled, default_command);
            assert_eq!(
                observed,
                cfg!(unix),
                "a Claude launch is observed on every Unix system, with {enabled} and {default_command}"
            );
            assert_eq!(
                continues,
                cfg!(unix) && enabled && default_command,
                "the continuation follows the panel on every Unix system"
            );
        }
    }

    #[test]
    fn observation_follows_the_launch_that_horizon_gives_its_arguments_to() {
        for kind in all_agent_kinds() {
            let (observed, _) = gates(*kind, true, true);
            assert_eq!(
                observed,
                cfg!(unix) && crate::agents::receives_horizon_arguments(*kind),
                "{kind:?}"
            );
        }
        for kind in [
            PanelKind::Shell,
            PanelKind::Ssh,
            PanelKind::Command,
            PanelKind::Browser,
            PanelKind::Editor,
            PanelKind::Device,
            PanelKind::Block,
        ] {
            let (observed, continues) = gates(kind, true, true);
            assert!(!observed && !continues, "{kind:?} runs no agent");
        }
    }

    #[test]
    fn a_launch_that_continues_needs_the_owned_provider_command() {
        let policy = ResumePolicy {
            enabled: true,
            ..ResumePolicy::default()
        };
        let launch = WorkLaunch {
            panel: "panel",
            kind: PanelKind::Claude,
            policy: &policy,
            cwd: None,
            session_id: Some("session"),
            default_command: true,
        };
        assert!(
            launch
                .attach(
                    None,
                    &mut ["-ic".to_string(), "claude".to_string()],
                    &mut HashMap::new()
                )
                .is_none(),
            "a continuation without an owned provider resolves no ledger"
        );
    }

    #[test]
    fn a_panel_with_a_command_of_its_own_is_observed_on_every_unix_system() {
        // No process-table read takes part in the report, so its gate names no
        // single system: Linux and macOS give the same answer.
        let policy = ResumePolicy::default();
        let launch = WorkLaunch {
            panel: "panel",
            kind: PanelKind::Claude,
            policy: &policy,
            cwd: None,
            session_id: Some("session"),
            default_command: false,
        };
        assert_eq!(launch.is_observed(), cfg!(unix));
        assert!(!launch.continues_work(), "a command of a preset never continues");
    }

    #[test]
    fn explicit_interrupt_remains_a_veto_despite_later_typed_input() {
        let directory = tempfile::tempdir().expect("fixture");
        let owner = WorkOwner {
            store: WorkStore::new(directory.path()),
            panel: "panel".into(),
            token: "owner".into(),
            interrupted: AtomicBool::new(false),
            continues: true,
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
            continues: true,
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
    fn a_launch_that_only_reports_writes_no_handoff() {
        let directory = tempfile::tempdir().expect("fixture");
        let path = directory.path().join("transcript.jsonl");
        std::fs::write(&path, "{\"type\":\"user\",\"message\":{\"content\":\"work\"}}\n").expect("transcript");
        let store = WorkStore::new(directory.path());
        store
            .register_owner("panel", PanelKind::Claude, "owner", Some("session"), directory.path())
            .expect("owner");
        let input: super::super::HookInput = serde_json::from_value(serde_json::json!({
            "session_id": "session", "prompt_id": "prompt", "hook_event_name": "UserPromptSubmit",
            "cwd": directory.path(), "transcript_path": path
        }))
        .expect("input");
        store
            .apply_hook("panel", PanelKind::Claude, "owner", &input, current_unix_millis())
            .expect("working");
        // The panel runs its turn under a policy that continues nothing, which is
        // the case of a report without a continuation.
        let owner = WorkOwner {
            store: store.clone(),
            panel: "panel".into(),
            token: "owner".into(),
            interrupted: AtomicBool::new(false),
            continues: false,
        };
        assert!(!owner.continues_work());
        assert!(
            !owner.prepare_suspend(),
            "a panel that only reports prepares no handoff"
        );
        owner.finish_suspend();
        let record = store.read("panel").expect("read").expect("record");
        assert!(record.handoff.is_none(), "no handoff is written");
        assert_eq!(
            record.ledger.state,
            TurnState::Working,
            "the ledger of the running turn stays as the agent reported it"
        );
    }

    #[test]
    fn plugin_path_is_always_shell_quoted() {
        assert_eq!(quote_argument("a;'b"), "'a;'\\''b'");
    }
}

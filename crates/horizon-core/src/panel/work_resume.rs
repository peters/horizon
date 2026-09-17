use crate::agent_work::WorkContinuation;
use crate::error::{Error, Result};

use super::{Panel, PanelKind};

pub(super) struct RestartWork {
    pub(super) owner: Option<std::sync::Arc<crate::agent_work::WorkOwner>>,
    pub(super) state: WorkContinuation,
}

impl Panel {
    /// Enable continuation on future launches. Disabling also removes pending
    /// offers and prevents cancellation for a continuation handoff at shutdown.
    pub fn set_work_resume_enabled(&mut self, enabled: bool) {
        self.work_resume.enabled = enabled;
        if !enabled && let Some(terminal) = self.terminal_mut() {
            terminal.work_owner = None;
            terminal.work_continuation = WorkContinuation::default();
        }
    }

    /// Authorize one explicit continuation; the ordinary restart path performs
    /// bounded teardown and submits the brief only to the same conversation.
    ///
    /// # Errors
    /// Rejects stale offers, ambiguous/custom launch commands, and providers
    /// without an exact session target.
    pub fn check_work_resume(&self) -> Result<()> {
        if !self.work_resume.enabled
            || self.remote_workspace.is_some()
            || self.launch_command.is_some()
            || !self.launch_args.is_empty()
            || !matches!(
                self.kind,
                PanelKind::Claude | PanelKind::Codex | PanelKind::OpenCode | PanelKind::Pi | PanelKind::Grok
            )
            || self
                .session_binding
                .as_ref()
                .is_none_or(|binding| binding.session_id.is_empty() || binding.kind != self.kind)
        {
            return Err(Error::State("This panel cannot safely restart into an exact conversation. Review it and continue from its terminal.".into()));
        }
        let terminal = self
            .terminal()
            .ok_or_else(|| Error::State("No terminal to resume".into()))?;
        if !terminal.work_continuation.owns_process {
            return Err(Error::State(
                "This process cannot be safely replaced. Continue from its terminal.".into(),
            ));
        }
        if terminal.pending_work_resume().is_none()
            || terminal.work_continuation.offered_session.as_deref()
                != self.session_binding.as_ref().map(|binding| binding.session_id.as_str())
        {
            return Err(Error::State(
                "The resume offer is no longer current. Review the terminal before continuing.".into(),
            ));
        }
        Ok(())
    }

    /// Authorize one continuation of the conversation that produced this offer.
    ///
    /// # Errors
    /// Returns the same refusal as [`Self::check_work_resume`].
    pub fn request_work_resume(&mut self) -> Result<()> {
        self.check_work_resume()?;
        let session_id = self
            .session_binding
            .as_ref()
            .map(|binding| binding.session_id.clone())
            .ok_or_else(|| Error::State("Missing conversation identity".into()))?;
        let terminal = self
            .terminal_mut()
            .ok_or_else(|| Error::State("No terminal to resume".into()))?;
        terminal.work_continuation.requested_session.get_or_insert(session_id);
        Ok(())
    }
    pub(super) fn preflight_restart_work(
        &self,
        program: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
        brief: Option<&str>,
    ) -> Result<Option<String>> {
        let owned = self
            .launch_args
            .is_empty()
            .then(|| self.work_launch().owned_command(program, args, env))
            .flatten();
        if brief.is_some() && owned.is_none() {
            return Err(Error::State(
                "The launch no longer resolves to an owned executable. Continue from its terminal.".into(),
            ));
        }
        if let Some(brief) = brief {
            self.append_work_brief(&mut args.to_vec(), brief)?;
        }
        Ok(owned)
    }

    pub(super) fn prepare_restart_work(
        &self,
        owned: Option<&str>,
        args: &mut Vec<String>,
        env: &mut std::collections::HashMap<String, String>,
        brief: Option<&str>,
    ) -> Result<RestartWork> {
        if brief.is_some() {
            self.check_work_session_exit()?;
        }
        let work_owner = self.work_launch().attach(owned, args, env);
        let work_state = self.prepare_work_process(args, owned);
        if let Some(brief) = brief {
            self.append_work_brief(args, brief)?;
        }
        Ok(RestartWork {
            owner: work_owner,
            state: work_state,
        })
    }

    fn work_launch(&self) -> crate::agent_work::WorkLaunch<'_> {
        crate::agent_work::WorkLaunch {
            panel: &self.local_id,
            kind: self.kind,
            policy: &self.work_resume,
            cwd: self.launch_cwd.as_deref(),
            session_id: self.session_binding.as_ref().map(|binding| binding.session_id.as_str()),
            default_command: self.launch_command.is_none(),
        }
    }

    pub(super) fn prepare_work_process(&self, args: &mut [String], owned: Option<&str>) -> WorkContinuation {
        let mut state = WorkContinuation::default();
        if self.work_resume.enabled
            && self.launch_command.is_none()
            && self.launch_args.is_empty()
            && self.kind.is_agent()
        {
            state.own_process(args, owned);
        }
        state
    }

    pub(super) fn requested_work_brief(&mut self) -> Result<Option<String>> {
        let Some(requested) = self
            .terminal_mut()
            .and_then(|terminal| terminal.work_continuation.requested_session.take())
        else {
            return Ok(None);
        };
        if self
            .session_binding
            .as_ref()
            .is_none_or(|binding| binding.session_id != requested)
        {
            return Err(Error::State(
                "The conversation changed after Resume work was selected.".into(),
            ));
        }
        self.check_work_resume()?;
        Ok(Some("The user selected Resume work in Horizon for this conversation. Re-read the latest user request and current conversation, then continue only the work already authorized. Re-verify the working tree, tool side effects, and background processes; interrupted tools may have partially completed. This does not answer pending questions or grant tool permissions. Ask the user if approval or a decision is still needed.".into()))
    }

    fn check_work_session_exit(&self) -> Result<()> {
        if self.kind == PanelKind::Claude
            && self
                .session_binding
                .as_ref()
                .is_some_and(|binding| crate::runtime_state::live_claude_session_ids().contains(&binding.session_id))
        {
            return Err(Error::State(
                "This conversation is still open in another process.".into(),
            ));
        }
        Ok(())
    }

    fn append_work_brief(&self, args: &mut Vec<String>, brief: &str) -> Result<()> {
        if !crate::agent_work::append_seed(self.kind, args, brief) {
            return Err(Error::State(
                "This launch cannot accept a continuation prompt. Continue from its terminal.".into(),
            ));
        }
        Ok(())
    }
}

pub(super) fn shutdown_for_restart(terminal: &mut crate::terminal::Terminal) -> Result<()> {
    if !terminal.shutdown_with_timeout(std::time::Duration::from_secs(2)) {
        return Err(Error::State(
            "The previous process has not finished shutting down. No continuation was started.".into(),
        ));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::agent_work::{ResumePolicy, StartupPlan, WorkLaunch};
    use crate::runtime_state::AgentSessionBinding;
    use crate::{PanelId, PanelOptions, WorkspaceId};

    #[test]
    fn confirmed_restart_replaces_the_process_and_submits_the_brief_once() {
        const CHILD_ENV: &str = "HORIZON_TEST_MANUAL_RESTART_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            exercise_confirmed_restart();
            return;
        }
        let home = tempfile::tempdir().expect("isolated home");
        let bin = home.path().join("bin");
        std::fs::create_dir(&bin).expect("fixture bin");
        std::fs::write(home.path().join(".bashrc"), "").expect("isolated shell config");
        let provider = bin.join("pi");
        std::fs::write(
            &provider,
            r#"#!/bin/bash
if test -f "$HOME/provider.pid"; then
    read -r previous < "$HOME/provider.pid"
    if kill -0 "$previous" 2>/dev/null; then
        printf 'overlapping processes\n' > "$HOME/overlap"
    fi
fi
printf '%s\0' "$@" > "$HOME/args.$$"
printf '%s\n' "$$" > "$HOME/provider.pid"
trap 'exit 0' HUP TERM
printf '%s\n' "$$" >> "$HOME/launches"
while :; do read -r -t 1 || :; done
"#,
        )
        .expect("fixture executable");
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700)).expect("executable permission");
        std::fs::copy(&provider, bin.join("claude")).expect("second disposable provider");
        // Re-exec isolates HOME, PATH and SHELL from concurrent tests and ensures
        // the ordinary launch resolver can only find our disposable provider.
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                concat!(
                    module_path!(),
                    "::confirmed_restart_replaces_the_process_and_submits_the_brief_once"
                )
                .trim_start_matches("horizon_core::"),
                "--nocapture",
            ])
            .env_clear()
            .env(CHILD_ENV, "1")
            .env("HOME", home.path())
            .env("SHELL", "/bin/bash")
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .current_dir(home.path())
            .output()
            .expect("isolated test process");
        assert!(
            output.status.success(),
            "isolated restart failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let launches = std::fs::read_to_string(home.path().join("launches")).expect("test child executed");
        assert_eq!(launches.lines().count(), 4);
        assert!(
            !home.path().join("overlap").exists(),
            "replacement started before previous process exited"
        );
    }

    fn exercise_confirmed_restart() {
        let home = std::path::PathBuf::from(std::env::var_os("HOME").expect("fixture home"));
        let mut panel = restored_provider(&home, PanelKind::Pi);
        assert_eq!(wait_for_launch(&home, 1), ["--session", "fixture-session"]);
        assert_refusal_preserves_process(&mut panel, &home, "owned executable", || {
            std::fs::write(home.join(".bashrc"), "pi() { :; }").expect("replace executable resolution");
        });
        std::fs::write(home.join(".bashrc"), "").expect("restore executable resolution");
        panel.request_work_resume().expect("confirm offered continuation");
        panel.restart().expect("confirmed restart");
        let args = wait_for_launch(&home, 2);
        assert_eq!(args.len(), 3, "exactly one brief must be submitted");
        assert_eq!(&args[..2], ["--session", "fixture-session"]);
        assert!(args[2].starts_with("The user selected Resume work in Horizon for this conversation."));
        assert!(panel.terminal().expect("terminal").pending_work_resume().is_none());
        assert!(panel.requested_work_brief().expect("consumed authorization").is_none());
        panel.restart().expect("ordinary restart after continuation");
        assert_eq!(wait_for_launch(&home, 3), ["--session", "fixture-session"]);
        shutdown_for_restart(panel.terminal_mut().expect("terminal")).expect("close disposable provider");

        let mut panel = restored_provider(&home, PanelKind::Claude);
        let _ = wait_for_launch(&home, 4);
        let project = home.join(".claude/projects/fixture");
        std::fs::create_dir_all(&project).expect("transcript directory");
        let transcript = project.join("fixture-session.jsonl");
        std::fs::write(&transcript, "{}").expect("saved transcript");
        assert!(crate::runtime_state::claude_session_transcript_exists(
            "fixture-session"
        ));
        assert_refusal_preserves_process(&mut panel, &home, "saved conversation", || {
            std::fs::remove_file(transcript).expect("remove transcript after confirmation");
        });
        shutdown_for_restart(panel.terminal_mut().expect("terminal")).expect("close missing-history provider");
    }

    fn assert_refusal_preserves_process(
        panel: &mut Panel,
        home: &std::path::Path,
        reason: &str,
        invalidate: impl FnOnce(),
    ) {
        let original_pid = std::fs::read_to_string(home.join("provider.pid")).expect("original PID");
        let launches = std::fs::read(home.join("launches")).expect("original launches");
        panel
            .request_work_resume()
            .expect("confirm before target becomes unavailable");
        invalidate();
        let error = panel.restart().expect_err("unavailable continuation must be refused");
        assert!(error.to_string().contains(reason), "unexpected refusal: {error}");
        assert!(
            std::process::Command::new("/bin/bash")
                .args(["-c", "kill -0 \"$1\"", "fixture-probe", original_pid.trim()])
                .status()
                .expect("probe original process")
                .success(),
            "refused continuation shut down the original process"
        );
        assert_eq!(std::fs::read(home.join("launches")).expect("launches"), launches);
        assert!(panel.terminal().expect("terminal").pending_work_resume().is_some());
        assert!(
            panel
                .requested_work_brief()
                .expect("refused authorization consumed")
                .is_none()
        );
    }

    fn restored_provider(home: &std::path::Path, kind: PanelKind) -> Panel {
        Panel::spawn(
            PanelId(1),
            WorkspaceId(1),
            PanelOptions {
                kind,
                cwd: Some(home.to_path_buf()),
                local_id: Some("manual-restart-fixture".into()),
                is_restore: true,
                work_resume: ResumePolicy {
                    enabled: true,
                    ..Default::default()
                },
                session_binding: Some(AgentSessionBinding::new(
                    kind,
                    "fixture-session".into(),
                    None,
                    None,
                    None,
                )),
                ..Default::default()
            },
        )
        .expect("restored disposable provider")
    }

    fn wait_for_launch(home: &std::path::Path, expected: usize) -> Vec<String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let launches = std::fs::read_to_string(home.join("launches")).unwrap_or_default();
            let pids: Vec<_> = launches.lines().collect();
            assert!(pids.len() <= expected, "unexpected additional provider launch");
            if pids.len() == expected && launches.ends_with('\n') {
                let bytes = std::fs::read(home.join(format!("args.{}", pids[expected - 1]))).expect("provider args");
                return bytes
                    .strip_suffix(&[0])
                    .expect("NUL terminated arguments")
                    .split(|byte| *byte == 0)
                    .map(|arg| String::from_utf8(arg.to_vec()).expect("UTF-8 argument"))
                    .collect();
            }
            assert!(std::time::Instant::now() < deadline, "provider did not launch");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn fixture() -> Panel {
        let mut panel = Panel::spawn(
            PanelId(1),
            WorkspaceId(1),
            PanelOptions {
                command: Some("/bin/sh".into()),
                args: vec!["-c".into(), "cat >/dev/null".into()],
                ..Default::default()
            },
        )
        .expect("disposable shell");
        panel.kind = PanelKind::Pi;
        panel.launch_command = None;
        panel.launch_args.clear();
        panel.session_binding = Some(AgentSessionBinding {
            kind: PanelKind::Pi,
            session_id: "fixture-session".into(),
            cwd: None,
            label: None,
            updated_at: None,
        });
        panel.work_resume = ResumePolicy {
            enabled: true,
            ..Default::default()
        };
        let plan = StartupPlan::prepare(
            &WorkLaunch {
                panel: &panel.local_id,
                kind: panel.kind,
                policy: &panel.work_resume,
                cwd: None,
                session_id: Some("fixture-session"),
                default_command: true,
            },
            true,
            true,
        );
        panel.terminal_mut().expect("terminal").work_continuation = plan.state;
        panel.terminal_mut().expect("terminal").work_continuation.owns_process = true;
        panel
    }
    #[test]
    fn manual_request_is_revoked_by_user_input_or_disabling_policy() {
        let mut panel = fixture();
        assert!(panel.request_work_resume().is_ok());
        assert!(panel.requested_work_brief().expect("brief").is_some());
        panel.request_work_resume().expect("request before cancellation");
        panel.terminal().expect("terminal").write_input(b"new request\n");
        assert!(panel.requested_work_brief().is_err());
        assert!(panel.requested_work_brief().expect("consumed refusal").is_none());
        panel.set_work_resume_enabled(false);
        assert!(panel.terminal().expect("terminal").pending_work_resume().is_none());
        assert!(panel.request_work_resume().is_err());
        shutdown_for_restart(panel.terminal_mut().expect("terminal")).expect("close fixture");
    }
    #[test]
    fn custom_arguments_and_missing_bindings_cannot_target_a_different_conversation() {
        let mut panel = fixture();
        panel.launch_args = vec!["--session".into(), "different".into()];
        assert!(panel.request_work_resume().is_err());
        panel.launch_args.clear();
        panel.session_binding = None;
        assert!(panel.request_work_resume().is_err());
        shutdown_for_restart(panel.terminal_mut().expect("terminal")).expect("close fixture");
    }
    #[test]
    fn changing_the_binding_revokes_the_offer_and_consumes_queued_authorization() {
        let mut panel = fixture();
        panel.request_work_resume().expect("request");
        panel.session_binding.as_mut().expect("binding").session_id = "different-session".into();
        assert!(panel.check_work_resume().is_err());
        assert!(panel.request_work_resume().is_err());
        assert!(panel.requested_work_brief().is_err());
        assert!(panel.requested_work_brief().expect("refusal consumed").is_none());
        shutdown_for_restart(panel.terminal_mut().expect("terminal")).expect("close fixture");
    }
}

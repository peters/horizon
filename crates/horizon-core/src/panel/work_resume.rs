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
    pub(super) fn prepare_restart_work(
        &self,
        program: &str,
        args: &mut Vec<String>,
        env: &mut std::collections::HashMap<String, String>,
        brief: Option<&str>,
    ) -> Result<RestartWork> {
        let work_launch = crate::agent_work::WorkLaunch {
            panel: &self.local_id,
            kind: self.kind,
            policy: &self.work_resume,
            cwd: self.launch_cwd.as_deref(),
            session_id: self.session_binding.as_ref().map(|binding| binding.session_id.as_str()),
            default_command: self.launch_command.is_none(),
        };
        let owned = self
            .launch_args
            .is_empty()
            .then(|| work_launch.owned_command(program, args, env))
            .flatten();
        if brief.is_some() && owned.is_none() {
            return Err(Error::State(
                "The launch no longer resolves to an owned executable. Continue from its terminal.".into(),
            ));
        }
        let work_owner = work_launch.attach(owned.as_deref(), args, env);
        let work_state = self.prepare_work_process(args, owned.as_deref());
        if let Some(brief) = brief {
            self.append_requested_work(args, brief)?;
        }
        Ok(RestartWork {
            owner: work_owner,
            state: work_state,
        })
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

    pub(super) fn append_requested_work(&self, args: &mut Vec<String>, brief: &str) -> Result<()> {
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
    use super::*;
    use crate::agent_work::{ResumePolicy, StartupPlan, WorkLaunch};
    use crate::runtime_state::AgentSessionBinding;
    use crate::{PanelId, PanelOptions, WorkspaceId};

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

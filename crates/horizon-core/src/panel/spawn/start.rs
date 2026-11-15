//! The reads that prepare a panel's process, and the plan that a worker gives
//! back to the frame.
//!
//! A panel's process needs the saved screen of its transcript (up to 8 MiB), its
//! working directory, the saved work of its agent and the probe of `script`.
//! Each of those reads a file or starts a process, so the frame asks the
//! coordination worker for a [`PanelStart`] and starts the process when the
//! answer arrives. A panel that waits shows the state of its start.

use std::collections::HashMap;

use crate::agent_work::StartupPlan;
use crate::error::Result;
use crate::runtime_state::AgentSessionBinding;
use crate::transcript::PanelTranscript;

use super::super::{Panel, PanelContent, PanelId, PanelKind, PanelOptions};
use super::{
    ResolvedTerminalLaunch, UnusableWorkingDirectory, WorkPanel, agent_env, prepare_transcript_restore,
    resolve_spawn_launch, resolve_terminal_launch, usable_working_directory,
};

/// What the start of a panel needs, read on a worker.
pub struct PanelStart {
    /// The directory that the panel cannot run in, when it cannot.
    pub(in crate::panel) working_directory: Option<UnusableWorkingDirectory>,
    /// The output that draws the saved screen again.
    pub(in crate::panel) replay_bytes: Vec<u8>,
    /// Whether the panel had a saved screen before it started.
    pub(in crate::panel) had_persisted_transcript_state: bool,
    /// The launch of the process, or `None` for a panel that runs none.
    pub(in crate::panel) launch: Option<PreparedLaunch>,
    /// The text of an editor panel that a worker read.
    pub(in crate::panel) editor_text: Option<std::result::Result<String, String>>,
    /// Whether the restart continues a saved conversation, so the old process
    /// must end before the new one starts.
    pub(in crate::panel) work_brief_wanted: bool,
}

impl PanelStart {
    /// The program and the arguments that the process of this panel starts with,
    /// or `None` for a panel that starts none.
    ///
    /// An agent panel starts through the login shell of this computer, so the
    /// vector names that shell and the one command string that the shell runs.
    #[must_use]
    pub fn launch_command(&self) -> Option<(&str, &[String])> {
        self.launch
            .as_ref()
            .map(|launch| (launch.program.as_str(), launch.args.as_slice()))
    }

    /// Whether the panel can start its process in its own directory.
    ///
    /// A panel whose directory was renamed or deleted cannot: it reports the
    /// directory instead of running in the directory of Horizon.
    #[must_use]
    pub fn working_directory_is_usable(&self) -> bool {
        self.working_directory.is_none()
    }
}

/// The launch of the process of a panel, resolved off the frame.
pub struct PreparedLaunch {
    pub(in crate::panel) session_binding: Option<AgentSessionBinding>,
    pub(in crate::panel) program: String,
    pub(in crate::panel) args: Vec<String>,
    pub(in crate::panel) env: HashMap<String, String>,
    pub(in crate::panel) work: StartupPlan,
    /// Whether the read of the named secrets of this start ended before this
    /// agent panel started, so it keeps no variable and says that in one line.
    pub(in crate::panel) named_secrets_late: bool,
}

/// The work that a start reads, and the values that it needs for it. The frame
/// copies them for a worker, because the panel itself stays on the frame.
pub struct StartRequest {
    pub id: PanelId,
    pub opts: PanelOptions,
    pub local_id: String,
}

/// Reads everything that the start of a panel needs. This is the work of the
/// coordination worker: the frame calls neither this function nor any read of
/// it.
#[must_use]
pub fn prepare_panel_start(request: StartRequest) -> PanelStart {
    let StartRequest { id, opts, local_id } = request;
    if opts.kind == PanelKind::Editor {
        // The panel opens at once; its text comes from its file on this worker.
        // A scratch buffer reads nothing and answers empty text.
        let editor_text = Some(
            opts.command
                .as_deref()
                .filter(|path| std::path::Path::new(path).exists())
                .map_or_else(
                    || Ok(String::new()),
                    |path| std::fs::read_to_string(path).map_err(|error| error.to_string()),
                ),
        );
        return PanelStart {
            working_directory: None,
            replay_bytes: Vec::new(),
            had_persisted_transcript_state: false,
            launch: None,
            editor_text,
            work_brief_wanted: false,
        };
    }
    let working_directory = working_directory_failure(&opts);
    let (transcript, replay_bytes, had_persisted_transcript_state) =
        prepare_transcript_restore(id, opts.kind, opts.transcript_root.clone(), &local_id);
    let launch = prepare_launch(id, &opts, &local_id, transcript.as_ref());
    PanelStart {
        working_directory,
        replay_bytes,
        had_persisted_transcript_state,
        launch: Some(launch),
        editor_text: None,
        work_brief_wanted: false,
    }
}

/// The directory this panel cannot start its process in, if any.
///
/// A panel whose directory was renamed or deleted must not keep Horizon's own
/// directory: its agent would work in another repository without a warning.
fn working_directory_failure(opts: &PanelOptions) -> Option<UnusableWorkingDirectory> {
    if !opts.runs_a_local_process() {
        return None;
    }
    let failure = usable_working_directory(opts.cwd.as_deref()).err()?;
    tracing::warn!(
        kind = ?opts.kind,
        directory = %failure.path,
        reason = %failure.reason,
        "panel cannot start in its working directory"
    );
    Some(failure)
}

fn prepare_launch(
    id: PanelId,
    opts: &PanelOptions,
    local_id: &str,
    transcript: Option<&PanelTranscript>,
) -> PreparedLaunch {
    let ResolvedTerminalLaunch {
        session_binding,
        program,
        mut launch_args,
    } = resolve_spawn_launch(id, opts, transcript);
    // The store of this start answers after the first frames, and this panel can
    // start before that answer. The wait is the worker's, bounded by the store.
    let named_secrets_published = super::wait_for_named_secrets(opts.kind);
    let mut env = agent_env(opts.kind, local_id, opts.command.is_none());
    let work = crate::agent_work::WorkLaunch {
        panel: local_id,
        kind: opts.kind,
        policy: &opts.work_resume,
        cwd: opts.cwd.as_deref(),
        session_id: session_binding.as_ref().map(|binding| binding.session_id.as_str()),
        default_command: opts.command.is_none(),
    }
    .start(
        &program,
        opts.is_restore,
        opts.args.is_empty(),
        &mut launch_args,
        &mut env,
    );
    PreparedLaunch {
        session_binding,
        program,
        args: launch_args,
        env,
        work,
        named_secrets_late: !named_secrets_published,
    }
}

/// The values that the pre-flight of a restart reads, copied from the panel for
/// a worker.
pub struct RestartRequest {
    pub(in crate::panel) id: PanelId,
    pub(in crate::panel) work: WorkPanel,
    /// The saved screen that a restart of a machine terminal keeps.
    pub(in crate::panel) replay_bytes: Vec<u8>,
    /// Whether the panel restarts into a saved conversation.
    pub(in crate::panel) work_brief: Option<String>,
}

/// The answer of the pre-flight of a restart. A panel that restarts into a
/// process waits for its own process to end before the launch resolves.
pub struct RestartPreflight {
    ready: Option<PanelStart>,
    pending: Option<PendingRestart>,
}

/// The launch of a restart that waits for the old process to end. The check that
/// the conversation is not open in any process any more reads the registry of
/// the provider, so it runs on a worker after the teardown.
pub struct PendingRestart {
    work: WorkPanel,
    working_directory: Option<UnusableWorkingDirectory>,
    replay_bytes: Vec<u8>,
    session_binding: Option<AgentSessionBinding>,
    program: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    owned: Option<String>,
    work_brief: Option<String>,
    /// Whether the read of the named secrets ended before this restart started.
    named_secrets_late: bool,
}

/// Reads the pre-flight of a restart and resolves its launch. This is the work
/// of the coordination worker; the frame restarts the panel with its answer.
///
/// # Errors
///
/// Returns the refusal that the resume policy gives: a saved conversation that
/// is gone, is open in another process, or cannot accept the continuation.
pub fn prepare_restart(request: RestartRequest) -> Result<RestartPreflight> {
    let RestartRequest {
        id,
        work,
        replay_bytes,
        work_brief,
    } = request;
    if let Some(editor) = &work.editor_file {
        let text = std::fs::read_to_string(editor).map_err(|error| error.to_string());
        return Ok(RestartPreflight {
            ready: Some(PanelStart {
                working_directory: None,
                replay_bytes: Vec::new(),
                had_persisted_transcript_state: false,
                launch: None,
                editor_text: Some(text),
                work_brief_wanted: false,
            }),
            pending: None,
        });
    }
    let working_directory = usable_working_directory(work.launch_cwd.as_deref()).err();
    // A pre-assigned Claude binding may not have a transcript yet (the panel
    // never received a message); resuming it would fail, so the panel relaunches
    // fresh under the same session id instead.
    let should_resume = work.kind.supports_session_binding()
        && work.session_binding.as_ref().is_some_and(|binding| {
            work.kind != PanelKind::Claude
                || crate::runtime_state::claude_session_transcript_exists(&binding.session_id)
        });
    let ResolvedTerminalLaunch {
        session_binding,
        program,
        launch_args,
    } = resolve_terminal_launch(
        id,
        work.kind,
        &work.resume,
        None,
        work.launch_command.clone(),
        work.launch_args.clone(),
        work.ssh_connection.clone(),
        work.session_binding.clone(),
        work.launch_cwd.as_ref(),
        None,
        true,
    );
    let named_secrets_published = super::wait_for_named_secrets(work.kind);
    let env = agent_env(work.kind, &work.local_id, work.launch_command.is_none());
    if work_brief.is_some() && !should_resume {
        return Err(crate::error::Error::State(
            "The saved conversation is no longer available.".into(),
        ));
    }
    let owned = work.preflight_restart_work(&program, &launch_args, &env, work_brief.as_deref())?;
    Ok(RestartPreflight {
        ready: None,
        pending: Some(PendingRestart {
            work,
            working_directory,
            replay_bytes,
            session_binding,
            program,
            args: launch_args,
            env,
            owned,
            work_brief,
            named_secrets_late: !named_secrets_published,
        }),
    })
}

impl RestartPreflight {
    /// The answer of a restart that needs no teardown, such as the new text of
    /// an editor panel, or `None` when the restart waits for the old process.
    pub fn take_ready(&mut self) -> Option<PanelStart> {
        self.ready.take()
    }

    /// The launch that waits for the old process to end.
    pub fn into_pending(self) -> Option<PendingRestart> {
        self.pending
    }

    /// Whether the restart continues a saved conversation, so the old process
    /// must end before the new one starts.
    pub fn work_brief_wanted(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.work_brief.is_some())
            || self.ready.as_ref().is_some_and(|ready| ready.work_brief_wanted)
    }
}

/// Resolves the launch of a restart after the old process ended: the work of the
/// coordination worker, which reads the registry of the provider once more.
///
/// # Errors
///
/// Refuses a conversation that another process still holds, and a launch that
/// cannot accept the continuation.
pub fn finish_restart(pending: PendingRestart) -> Result<PanelStart> {
    let PendingRestart {
        work,
        working_directory,
        replay_bytes,
        session_binding,
        program,
        mut args,
        mut env,
        owned,
        work_brief,
        named_secrets_late,
    } = pending;
    let crate::panel::work_resume::RestartWork { owner, state } =
        work.prepare_restart_work(owned.as_deref(), &mut args, &mut env, work_brief.as_deref())?;
    Ok(PanelStart {
        working_directory,
        replay_bytes,
        had_persisted_transcript_state: false,
        launch: Some(PreparedLaunch {
            session_binding,
            program,
            args,
            env,
            work: StartupPlan::settled(owner, state),
            named_secrets_late,
        }),
        editor_text: None,
        work_brief_wanted: work_brief.is_some(),
    })
}

impl Panel {
    /// The values that the pre-flight of a restart reads, copied for a worker.
    /// `None` when this panel restarts nothing: a page relaunches itself here,
    /// as it reads no file, and a panel that runs no process has nothing to
    /// start again.
    ///
    /// # Errors
    ///
    /// Returns the refusal of a resume that the panel can no longer offer.
    pub fn restart_request(&mut self) -> Result<Option<RestartRequest>> {
        if let PanelContent::Browser(browser) = &mut self.content {
            browser.relaunch();
            return Ok(None);
        }
        if !matches!(self.content, PanelContent::Terminal(_) | PanelContent::Editor(_)) {
            return Ok(None);
        }
        let work_brief = self.requested_work_brief()?;
        let work = self.work_panel();
        Ok(Some(RestartRequest {
            id: self.id,
            work,
            replay_bytes: self.restart_replay_bytes(),
            work_brief,
        }))
    }
}

//! Cached, asynchronous executable discovery for Speech Input setup agents.

mod probe;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use horizon_core::Config;

pub(super) use self::probe::verify_preset_command;
use self::probe::{AgentScanKey, ProbeEnvironment, login_shell_probe_timeout, probe_candidate};
use super::SpeechSetupAgent;

const COMPLETE_SCAN_TIMEOUT: Duration = Duration::from_millis(1_750);

/// UI-facing state for one setup agent. Failures and timeouts stay unknown;
/// they must never turn into a false `Available` or a definitive `Missing`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum SpeechSetupAgentAvailability {
    Checking,
    Available { executable: String },
    Missing,
    Unknown(SpeechSetupProbeFailure),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum SpeechSetupProbeFailure {
    Timeout,
    Failed(String),
}

impl SpeechSetupProbeFailure {
    pub(super) fn user_message(&self) -> &str {
        match self {
            Self::Timeout => "detection timed out; availability is unknown",
            Self::Failed(_) => "detection failed; availability is unknown",
        }
    }
}

struct AgentProbeMessage {
    generation: u64,
    agent: SpeechSetupAgent,
    availability: SpeechSetupAgentAvailability,
}

pub(super) struct AgentProbeCache {
    key: Option<AgentScanKey>,
    generation: u64,
    codex: SpeechSetupAgentAvailability,
    claude: SpeechSetupAgentAvailability,
    receiver: Option<Receiver<AgentProbeMessage>>,
    started_at: Option<Instant>,
}

impl AgentProbeCache {
    pub(super) const fn new() -> Self {
        Self {
            key: None,
            generation: 0,
            codex: SpeechSetupAgentAvailability::Checking,
            claude: SpeechSetupAgentAvailability::Checking,
            receiver: None,
            started_at: None,
        }
    }

    pub(super) fn sync(&mut self, ctx: &egui::Context, config: &Config, workspace_cwd: Option<&Path>) {
        let environment = ProbeEnvironment::capture(workspace_cwd);
        let key = AgentScanKey::new(config, &environment);
        if self.key.as_ref() == Some(&key) {
            self.poll(ctx);
        } else {
            self.start_scan(ctx, &key, &environment);
        }
    }

    pub(super) fn invalidate(&mut self) {
        self.key = None;
        self.receiver = None;
        self.started_at = None;
        self.codex = SpeechSetupAgentAvailability::Checking;
        self.claude = SpeechSetupAgentAvailability::Checking;
    }

    pub(super) fn availability(&self, agent: SpeechSetupAgent) -> SpeechSetupAgentAvailability {
        match agent {
            SpeechSetupAgent::Codex => self.codex.clone(),
            SpeechSetupAgent::Claude => self.claude.clone(),
        }
    }

    pub(super) fn resolved_command(&self, agent: SpeechSetupAgent) -> Option<String> {
        match self.availability(agent) {
            SpeechSetupAgentAvailability::Available { executable } => Some(executable),
            SpeechSetupAgentAvailability::Checking
            | SpeechSetupAgentAvailability::Missing
            | SpeechSetupAgentAvailability::Unknown(_) => None,
        }
    }

    #[cfg(test)]
    pub(super) fn set_test_availability(
        &mut self,
        codex: SpeechSetupAgentAvailability,
        claude: SpeechSetupAgentAvailability,
    ) {
        self.codex = codex;
        self.claude = claude;
    }

    fn start_scan(&mut self, ctx: &egui::Context, key: &AgentScanKey, environment: &ProbeEnvironment) {
        self.generation = self.generation.wrapping_add(1);
        self.key = Some(key.clone());
        self.codex = SpeechSetupAgentAvailability::Checking;
        self.claude = SpeechSetupAgentAvailability::Checking;
        self.started_at = Some(Instant::now());

        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);

        for agent in SpeechSetupAgent::ALL {
            let Some(command) = key.command(agent).map(str::to_string) else {
                self.set_availability(
                    agent,
                    SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(format!(
                        "no default {} preset is available",
                        agent.display_name()
                    ))),
                );
                continue;
            };
            let probe_sender = sender.clone();
            let probe_environment = environment.clone();
            let repaint_ctx = ctx.clone();
            let generation = self.generation;
            let thread_name = format!("speech-setup-{}-probe", agent.display_name().to_ascii_lowercase());
            let spawn_result = std::thread::Builder::new().name(thread_name).spawn(move || {
                let availability = probe_candidate(&command, &probe_environment, login_shell_probe_timeout());
                let _ = probe_sender.send(AgentProbeMessage {
                    generation,
                    agent,
                    availability,
                });
                repaint_ctx.request_repaint();
            });
            if let Err(error) = spawn_result {
                self.set_availability(
                    agent,
                    SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(format!(
                        "failed to start executable probe: {error}"
                    ))),
                );
            }
        }
        drop(sender);

        ctx.request_repaint_after(COMPLETE_SCAN_TIMEOUT);
        self.poll(ctx);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let mut messages = Vec::new();
        let mut disconnected = false;
        if let Some(receiver) = self.receiver.as_ref() {
            loop {
                match receiver.try_recv() {
                    Ok(message) => messages.push(message),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        for message in messages {
            if message.generation == self.generation {
                self.set_availability(message.agent, message.availability);
            }
        }

        let timed_out = self
            .started_at
            .is_some_and(|started_at| started_at.elapsed() >= COMPLETE_SCAN_TIMEOUT);
        if timed_out {
            self.replace_checking_with_unknown(SpeechSetupProbeFailure::Timeout);
            self.receiver = None;
            self.started_at = None;
        } else if disconnected {
            self.replace_checking_with_unknown(SpeechSetupProbeFailure::Failed(
                "probe worker disconnected before reporting a result".to_string(),
            ));
            self.receiver = None;
            self.started_at = None;
        } else if self.all_terminal() {
            self.receiver = None;
            self.started_at = None;
        } else if let Some(started_at) = self.started_at {
            let remaining = COMPLETE_SCAN_TIMEOUT.saturating_sub(started_at.elapsed());
            ctx.request_repaint_after(remaining);
        }
    }

    fn replace_checking_with_unknown(&mut self, failure: SpeechSetupProbeFailure) {
        if matches!(self.codex, SpeechSetupAgentAvailability::Checking) {
            self.codex = SpeechSetupAgentAvailability::Unknown(failure.clone());
        }
        if matches!(self.claude, SpeechSetupAgentAvailability::Checking) {
            self.claude = SpeechSetupAgentAvailability::Unknown(failure);
        }
    }

    fn set_availability(&mut self, agent: SpeechSetupAgent, availability: SpeechSetupAgentAvailability) {
        match agent {
            SpeechSetupAgent::Codex => self.codex = availability,
            SpeechSetupAgent::Claude => self.claude = availability,
        }
    }

    fn all_terminal(&self) -> bool {
        !matches!(self.codex, SpeechSetupAgentAvailability::Checking)
            && !matches!(self.claude, SpeechSetupAgentAvailability::Checking)
    }
}

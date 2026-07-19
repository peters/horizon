//! The speech state machine that the frame loop talks to.

use horizon_core::{PanelId, ShortcutBinding, SpeechConfig, SpeechHotkeyMode};

use super::capture::{CaptureCmd, CaptureHandle};
use super::worker::{Job, WorkerEvent, WorkerHandle};
use super::{MicState, SpeechEvent};

/// Recordings shorter than this are dropped as accidental taps.
const MIN_PCM_SAMPLES: usize = 4_000; // 0.25 s at 16 kHz

enum State {
    Idle,
    Recording { target: PanelId },
    Busy { target: PanelId },
}

pub struct SpeechSystem {
    capture: CaptureHandle,
    worker: WorkerHandle,
    state: State,
    binding: Option<ShortcutBinding>,
    hotkey_mode: SpeechHotkeyMode,
}

impl SpeechSystem {
    /// Build from config; `None` when the feature is disabled in config.
    #[must_use]
    pub fn from_config(config: &SpeechConfig) -> Option<Self> {
        if !config.enabled {
            return None;
        }
        let binding = match config.hotkey.trim() {
            "" => None,
            hotkey => match ShortcutBinding::parse(hotkey) {
                Ok(binding) => Some(binding),
                Err(error) => {
                    tracing::warn!(%error, hotkey, "invalid speech hotkey; push-to-talk disabled");
                    None
                }
            },
        };
        Some(Self {
            capture: CaptureHandle::spawn(),
            worker: WorkerHandle::spawn(config),
            state: State::Idle,
            binding,
            hotkey_mode: config.hotkey_mode,
        })
    }

    #[must_use]
    pub fn hotkey_binding(&self) -> Option<ShortcutBinding> {
        self.binding
    }

    #[must_use]
    pub fn hotkey_mode(&self) -> SpeechHotkeyMode {
        self.hotkey_mode
    }

    #[must_use]
    pub fn mic_state_for(&self, panel: PanelId) -> MicState {
        match self.state {
            State::Recording { target } if target == panel => MicState::Recording,
            State::Busy { target } if target == panel => MicState::Busy,
            _ => MicState::Idle,
        }
    }

    #[must_use]
    pub fn recording_target(&self) -> Option<PanelId> {
        match self.state {
            State::Recording { target } => Some(target),
            _ => None,
        }
    }

    /// Recording or transcribing — the frame loop keeps repainting (and thus
    /// polling) while this is true.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// Mic-button semantics: start when idle, stop when this panel records.
    pub fn toggle(&mut self, target: PanelId) {
        match self.state {
            State::Idle => self.start(target),
            State::Recording { target: current } if current == target => self.stop(),
            // Recording another panel or transcription in flight: ignore.
            State::Recording { .. } | State::Busy { .. } => {}
        }
    }

    pub fn start(&mut self, target: PanelId) {
        if matches!(self.state, State::Idle) {
            self.capture.send(CaptureCmd::Start);
            self.state = State::Recording { target };
        }
    }

    pub fn stop(&mut self) {
        if let State::Recording { target } = self.state {
            self.capture.send(CaptureCmd::Stop);
            self.state = State::Busy { target };
        }
    }

    pub fn cancel(&mut self) {
        if matches!(self.state, State::Recording { .. }) {
            self.capture.send(CaptureCmd::Cancel);
            self.state = State::Idle;
        }
    }

    /// Drain worker/capture channels; called once per frame.
    pub fn poll(&mut self) -> Vec<SpeechEvent> {
        let mut events = Vec::new();

        while let Some(pcm) = self.capture.try_recv_pcm() {
            match pcm {
                Ok(pcm) => {
                    if let State::Busy { target } = self.state {
                        if pcm.len() < MIN_PCM_SAMPLES {
                            tracing::debug!(samples = pcm.len(), "speech recording too short; dropped");
                            self.state = State::Idle;
                        } else {
                            self.worker.submit(Job { pcm, target });
                        }
                    }
                }
                Err(message) => {
                    self.state = State::Idle;
                    events.push(SpeechEvent::Error(message));
                }
            }
        }

        while let Some(event) = self.worker.try_recv_event() {
            self.state = State::Idle;
            match event {
                WorkerEvent::Done { target, text } => {
                    if !text.is_empty() {
                        events.push(SpeechEvent::Text { target, text });
                    }
                }
                WorkerEvent::Failed { message } => events.push(SpeechEvent::Error(message)),
            }
        }

        events
    }
}

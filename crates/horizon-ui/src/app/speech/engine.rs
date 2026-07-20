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
    /// Monotonic recording generation; results tagged with an older value
    /// are stale (from a cancelled/failed recording) and are ignored.
    generation: u64,
    /// Backend the loaded model actually runs on (known after first use).
    active_backend: Option<String>,
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
        let capture = match CaptureHandle::spawn() {
            Ok(capture) => capture,
            Err(error) => {
                tracing::error!(%error, "failed to start speech capture thread; speech input disabled");
                return None;
            }
        };
        let worker = match WorkerHandle::spawn(config) {
            Ok(worker) => worker,
            Err(error) => {
                tracing::error!(%error, "failed to start speech transcription thread; speech input disabled");
                return None;
            }
        };
        Some(Self {
            capture,
            worker,
            state: State::Idle,
            binding,
            hotkey_mode: config.hotkey_mode,
            generation: 0,
            active_backend: None,
        })
    }

    /// The backend the model actually selected, once known (useful when the
    /// configured backend is `auto`).
    #[must_use]
    pub fn active_backend(&self) -> Option<&str> {
        self.active_backend.as_deref()
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
            self.generation += 1;
            self.capture.send(CaptureCmd::Start(self.generation));
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

        while let Some((generation, pcm)) = self.capture.try_recv_pcm() {
            if generation != self.generation {
                tracing::debug!(generation, current = self.generation, "stale capture result ignored");
                continue;
            }
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
                    // A stream that died mid-recording leaves the capture
                    // thread holding a broken stream; tear it down too.
                    if matches!(self.state, State::Recording { .. }) {
                        self.capture.send(CaptureCmd::Cancel);
                    }
                    self.state = State::Idle;
                    events.push(SpeechEvent::Error(message));
                }
            }
        }

        while let Some(event) = self.worker.try_recv_event() {
            match event {
                WorkerEvent::ModelLoaded { backend } => {
                    self.active_backend = Some(backend);
                }
                WorkerEvent::Done { target, text } => {
                    self.state = State::Idle;
                    if !text.is_empty() {
                        events.push(SpeechEvent::Text { target, text });
                    }
                }
                WorkerEvent::Failed { message } => {
                    self.state = State::Idle;
                    events.push(SpeechEvent::Error(message));
                }
            }
        }

        events
    }
}

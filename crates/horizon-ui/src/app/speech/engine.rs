//! The speech state machine that the frame loop talks to.
//!
//! Multiple named profiles run side by side, each owning a worker (and thus
//! its lazily loaded model) and an optional dedicated push-to-talk key —
//! hold F1 to dictate Norwegian, F2 for English, with no mode switching.
//! The microphone itself is shared: one recording at a time, attributed to
//! the profile whose key (or the mic button's last-used profile) started it.

use horizon_core::{PanelId, ShortcutBinding, SpeechConfig, SpeechHotkeyMode};

use super::capture::{CaptureCmd, CaptureHandle};
use super::worker::{Job, WorkerEvent, WorkerHandle};
use super::{MicState, SpeechEvent};

/// Recordings shorter than this are dropped as accidental taps.
const MIN_PCM_SAMPLES: usize = 4_000; // 0.25 s at 16 kHz

enum State {
    Idle,
    Recording {
        target: PanelId,
        profile: usize,
    },
    /// Mic stopped; awaiting the captured PCM from the capture thread. A
    /// capture error here still returns to Idle.
    AwaitingPcm {
        target: PanelId,
        profile: usize,
    },
    /// The worker owns the job; only its Done/Failed returns to Idle, and a
    /// late capture stream-error is ignored.
    Transcribing {
        target: PanelId,
        profile: usize,
    },
}

struct ProfileRuntime {
    label: String,
    binding: Option<ShortcutBinding>,
    worker: WorkerHandle,
}

pub struct SpeechSystem {
    capture: CaptureHandle,
    profiles: Vec<ProfileRuntime>,
    /// Cached `(profile index, binding)` pairs — bindings are immutable for
    /// the lifetime of a `SpeechSystem`, and the frame path reads them every
    /// frame (hot path: no per-frame allocation).
    resolved_bindings: Vec<(usize, ShortcutBinding)>,
    state: State,
    hotkey_mode: SpeechHotkeyMode,
    /// Monotonic recording generation; results tagged with an older value
    /// are stale (from a cancelled/failed recording) and are ignored.
    generation: u64,
    /// Backend the most recently loaded model runs on.
    active_backend: Option<String>,
    /// Profile used by the mic button (the hotkeys address theirs directly).
    last_used: usize,
}

impl SpeechSystem {
    /// Build from config; `None` when the feature is disabled in config.
    #[must_use]
    pub fn from_config(config: &SpeechConfig) -> Option<Self> {
        if !config.enabled {
            return None;
        }
        let capture = match CaptureHandle::spawn() {
            Ok(capture) => capture,
            Err(error) => {
                tracing::error!(%error, "failed to start speech capture thread; speech input disabled");
                return None;
            }
        };
        let mut profiles = Vec::new();
        for (index, profile) in config.resolved_profiles().iter().enumerate() {
            let label = if profile.name.trim().is_empty() {
                format!("profile {}", index + 1)
            } else {
                profile.name.trim().to_string()
            };
            let binding = match profile.hotkey.trim() {
                "" => None,
                hotkey => match ShortcutBinding::parse(hotkey) {
                    Ok(binding) => Some(binding),
                    Err(error) => {
                        tracing::warn!(%error, hotkey, label, "invalid speech hotkey; push-to-talk disabled for profile");
                        None
                    }
                },
            };
            let worker = match WorkerHandle::spawn(profile, config.backend) {
                Ok(worker) => worker,
                Err(error) => {
                    tracing::error!(%error, label, "failed to start speech transcription thread; speech input disabled");
                    return None;
                }
            };
            profiles.push(ProfileRuntime { label, binding, worker });
        }
        let resolved_bindings = profiles
            .iter()
            .enumerate()
            .filter_map(|(index, profile)| profile.binding.map(|binding| (index, binding)))
            .collect();
        Some(Self {
            capture,
            profiles,
            resolved_bindings,
            state: State::Idle,
            hotkey_mode: config.hotkey_mode,
            generation: 0,
            active_backend: None,
            last_used: 0,
        })
    }

    /// The backend the most recently loaded model selected (useful when the
    /// configured backend is `auto`).
    #[must_use]
    pub fn active_backend(&self) -> Option<&str> {
        self.active_backend.as_deref()
    }

    /// Every profile's push-to-talk binding, by profile index.
    #[must_use]
    pub fn profile_bindings(&self) -> &[(usize, ShortcutBinding)] {
        &self.resolved_bindings
    }

    /// Human-readable key summary for tooltips, e.g. `F1 Norsk · F2 English`.
    #[must_use]
    pub fn hotkey_summary(&self, primary_label: &str) -> Option<String> {
        let parts: Vec<String> = self
            .profiles
            .iter()
            .filter_map(|profile| {
                profile
                    .binding
                    .map(|binding| format!("{} {}", binding.display_label(primary_label), profile.label))
            })
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" · "))
        }
    }

    #[must_use]
    pub fn hotkey_mode(&self) -> SpeechHotkeyMode {
        self.hotkey_mode
    }

    #[must_use]
    pub fn mic_state_for(&self, panel: PanelId) -> MicState {
        match self.state {
            State::Recording { target, .. } if target == panel => MicState::Recording,
            State::AwaitingPcm { target, .. } | State::Transcribing { target, .. } if target == panel => MicState::Busy,
            _ => MicState::Idle,
        }
    }

    #[must_use]
    pub fn recording_target(&self) -> Option<PanelId> {
        match self.state {
            State::Recording { target, .. } => Some(target),
            _ => None,
        }
    }

    /// The target panel of any active state (recording, awaiting PCM, or
    /// transcribing), for the "target must still exist" invariant.
    #[must_use]
    pub fn active_target(&self) -> Option<PanelId> {
        match self.state {
            State::Recording { target, .. }
            | State::AwaitingPcm { target, .. }
            | State::Transcribing { target, .. } => Some(target),
            State::Idle => None,
        }
    }

    /// Recording or transcribing — the frame loop keeps repainting (and thus
    /// polling) while this is true.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// Mic-button semantics: start (with the last-used profile) when idle,
    /// stop when this panel is recording.
    pub fn toggle(&mut self, target: PanelId) {
        match self.state {
            State::Idle => self.start(target, self.last_used),
            State::Recording { target: current, .. } if current == target => self.stop(),
            // Recording another panel or transcription in flight: ignore.
            State::Recording { .. } | State::AwaitingPcm { .. } | State::Transcribing { .. } => {}
        }
    }

    pub fn start(&mut self, target: PanelId, profile: usize) {
        if matches!(self.state, State::Idle) && profile < self.profiles.len() {
            self.generation += 1;
            self.last_used = profile;
            self.capture.send(CaptureCmd::Start(self.generation));
            self.state = State::Recording { target, profile };
        }
    }

    pub fn stop(&mut self) {
        if let State::Recording { target, profile } = self.state {
            self.capture.send(CaptureCmd::Stop);
            self.state = State::AwaitingPcm { target, profile };
        }
    }

    /// Return to Idle from any active state. A running transcription cannot
    /// be interrupted, but the engine is freed so the mic/hotkey work again;
    /// its eventual result is discarded because the state no longer matches.
    pub fn cancel(&mut self) {
        if matches!(self.state, State::Recording { .. }) {
            self.capture.send(CaptureCmd::Cancel);
        }
        self.state = State::Idle;
    }

    /// Drain worker/capture channels; called once per frame.
    pub fn poll(&mut self) -> Vec<SpeechEvent> {
        let mut events = Vec::new();

        // Prove the frame loop is alive so the capture thread keeps the mic
        // open; if frames stop, its heartbeat goes stale and it self-cancels.
        if matches!(self.state, State::Recording { .. }) {
            self.capture.heartbeat();
        }

        while let Some((generation, pcm)) = self.capture.try_recv_pcm() {
            if generation != self.generation {
                tracing::debug!(generation, current = self.generation, "stale capture result ignored");
                continue;
            }
            match pcm {
                Ok(pcm) => {
                    // AwaitingPcm is the normal stop path; Recording happens
                    // when the capture thread auto-finalized at the length
                    // cap without a Stop command. Both deliver the captured
                    // audio to a worker.
                    let pending = match self.state {
                        State::AwaitingPcm { target, profile } | State::Recording { target, profile } => {
                            Some((target, profile))
                        }
                        _ => None,
                    };
                    if let Some((target, profile)) = pending {
                        if pcm.len() < MIN_PCM_SAMPLES {
                            tracing::debug!(samples = pcm.len(), "speech recording too short; dropped");
                            self.state = State::Idle;
                        } else if let Some(runtime) = self.profiles.get(profile) {
                            runtime.worker.submit(Job { pcm, target });
                            self.state = State::Transcribing { target, profile };
                        } else {
                            self.state = State::Idle;
                        }
                    }
                }
                Err(message) => {
                    // A capture error before the PCM reached a worker
                    // (Recording, or AwaitingPcm after a quick stop where
                    // start actually failed) must return to Idle — no worker
                    // event will. Once Transcribing, the worker owns the job
                    // and a late stream-error is ignored.
                    match self.state {
                        State::Recording { .. } => {
                            self.capture.send(CaptureCmd::Cancel);
                            self.state = State::Idle;
                            events.push(SpeechEvent::Error(message));
                        }
                        State::AwaitingPcm { .. } => {
                            self.state = State::Idle;
                            events.push(SpeechEvent::Error(message));
                        }
                        _ => tracing::debug!(%message, "late capture error ignored"),
                    }
                }
            }
        }

        for index in 0..self.profiles.len() {
            while let Some(event) = self.profiles[index].worker.try_recv_event() {
                match event {
                    WorkerEvent::ModelLoaded { backend } => {
                        self.active_backend = Some(backend);
                    }
                    WorkerEvent::Done { target, text } => {
                        if matches!(self.state, State::Transcribing { profile, .. } if profile == index) {
                            self.state = State::Idle;
                        }
                        if !text.is_empty() {
                            events.push(SpeechEvent::Text { target, text });
                        }
                    }
                    WorkerEvent::Failed { message } => {
                        if matches!(self.state, State::Transcribing { profile, .. } if profile == index) {
                            self.state = State::Idle;
                        }
                        events.push(SpeechEvent::Error(message));
                    }
                }
            }
        }

        events
    }
}

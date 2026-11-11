//! The hotkey, the insert and the state mark of speech input.
//!
//! The window sends one key event to the worker thread of
//! [`horizon_core::speech`] and reads its events: the recorder runs while the
//! hotkey is down, and the text of the take goes into the panel that had the focus
//! when the key went down, through the paste path of the terminal.
//!
//! The state mark is a small pill of the panel of the take. It is drawn while a
//! take records and for six seconds after a failed one, and it repaints once per
//! second while the take records: a quiet window pays nothing for it.

mod pill;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Context, Key};
use horizon_core::named_secrets::NamedSecretVault;
use horizon_core::speech::{SpeechEventKind, SpeechInput, SpeechWorker, TakePlan};
use horizon_core::{PanelId, ShortcutBinding, ShortcutKey, ShortcutModifiers};

use super::HorizonApp;
use super::shortcuts::{key_pressed_in_events, shortcut_pressed_in_events, shortcut_released_in_events};
use crate::theme;

pub(in crate::app) use pill::MarkTone;

/// How long the sentence of a failed take stays on the panel.
const ERROR_MARK: Duration = Duration::from_secs(6);

/// How often the mark repaints while a take records.
const LISTENING_TICK: Duration = Duration::from_secs(1);

/// How long the take of the **Try** button of the Settings tab records.
const TRY_TAKE: Duration = Duration::from_secs(2);

/// The sentence of a hotkey that no terminal can receive.
pub(super) const NO_TERMINAL: &str = "Click a terminal first.";

/// The sentence of a hotkey while the settings name no recorder or no transcriber.
pub(super) const NOT_SET: &str = "Speech input is off. Set a recorder and a transcriber in Settings.";

/// The panel of a take that the Settings tab runs, so no panel receives its text.
pub(super) const SETTINGS_TRY: PanelId = PanelId(u64::MAX);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Listening,
    Writing,
}

/// The state of speech input in one window.
pub(in crate::app) struct SpeechUi {
    /// The worker thread of this window, started at its first take.
    input: Option<SpeechInput>,
    /// The root of the Horizon home, where the takes live. It comes with the first
    /// take, because the home of the window is known after the window exists.
    home: Option<PathBuf>,
    /// The named secrets of this computer. The worker reads the value of the key
    /// from this one, on its own thread, at the moment of the request.
    keys: Option<Arc<NamedSecretVault>>,
    /// The hotkey of the settings, or `None` while the settings name none.
    binding: Option<ShortcutBinding>,
    phase: Phase,
    /// The panel that had the focus when the hotkey went down.
    target: Option<PanelId>,
    /// When the take started to record, for the seconds of the mark.
    started: Option<Instant>,
    /// The sentence of a failed take and the instant that it goes.
    error: Option<(String, Instant)>,
    /// When the take of the **Try** button started, so the window ends it.
    try_started: Option<Instant>,
    /// The answer of the **Try** button of the Settings tab.
    try_answer: Option<Result<String, String>>,
    /// The name of the key whose state the window read last, so a frame asks the
    /// secrets worker once for one name.
    asked_key: Option<horizon_core::named_secrets::SecretName>,
}

impl SpeechUi {
    pub(in crate::app) fn new() -> Self {
        Self {
            input: None,
            home: None,
            keys: None,
            binding: None,
            phase: Phase::Idle,
            target: None,
            started: None,
            error: None,
            try_started: None,
            try_answer: None,
            asked_key: None,
        }
    }

    /// The hotkey of `config`, and the named secrets that its key lives in.
    pub(in crate::app) fn configure(&mut self, config: &horizon_core::Config, keys: Option<Arc<NamedSecretVault>>) {
        self.keys = keys;
        let hotkey = config.speech.hotkey.trim();
        self.binding = (!hotkey.is_empty())
            .then(|| ShortcutBinding::parse(hotkey).ok())
            .flatten();
    }

    fn is_idle(&self) -> bool {
        self.phase == Phase::Idle
    }

    fn is_listening(&self) -> bool {
        self.phase == Phase::Listening
    }

    /// The panel of the take that the mark is drawn on.
    pub(in crate::app) fn target(&self) -> Option<PanelId> {
        self.target
    }

    /// The line of the mark at `now`, with the color of its tone.
    pub(in crate::app) fn mark(&self, now: Instant) -> Option<(String, MarkTone)> {
        if let Some((message, until)) = &self.error
            && now < *until
        {
            return Some((message.clone(), MarkTone::Error));
        }
        match self.phase {
            Phase::Idle => None,
            Phase::Listening => {
                let elapsed = self
                    .started
                    .map_or(Duration::ZERO, |started| now.saturating_duration_since(started));
                Some((format!("Listening {}", clock_text(elapsed)), MarkTone::Listening))
            }
            Phase::Writing => Some(("Writing…".to_owned(), MarkTone::Writing)),
        }
    }

    /// The name of the key whose state the window read last.
    pub(in crate::app) fn asked_key(&self) -> Option<&horizon_core::named_secrets::SecretName> {
        self.asked_key.as_ref()
    }

    /// Keep the name of the key whose state the window read.
    pub(in crate::app) fn set_asked_key(&mut self, name: horizon_core::named_secrets::SecretName) {
        self.asked_key = Some(name);
    }

    /// The answer of the **Try** button of the Settings tab, and whether it waits.
    pub(in crate::app) fn try_answer(&self) -> Option<&Result<String, String>> {
        self.try_answer.as_ref()
    }

    pub(in crate::app) fn try_running(&self) -> bool {
        self.try_started.is_some()
    }

    /// Start one take of `plan` into `target`.
    fn begin(&mut self, ctx: &Context, plan: TakePlan, target: PanelId, home: &Path) -> bool {
        if !self.is_idle() {
            return false;
        }
        let home = self.home.get_or_insert_with(|| home.to_path_buf()).clone();
        let wake_context = ctx.clone();
        let keys = self.keys_reader();
        let input = self.input.get_or_insert_with(move || {
            SpeechInput::start(
                SpeechWorker::new(home, keys).with_wake(Arc::new(move || wake_context.request_repaint())),
            )
        });
        input.begin(plan, target);
        self.phase = Phase::Listening;
        self.target = Some(target);
        self.started = Some(Instant::now());
        self.error = None;
        true
    }

    fn keys_reader(&self) -> Arc<dyn horizon_core::speech::SpeechKeyStore> {
        match self.keys.clone() {
            Some(keys) => keys,
            None => Arc::new(()),
        }
    }

    /// One event of the worker, or `None` while it has none.
    fn poll(&self) -> Option<horizon_core::speech::SpeechEvent> {
        self.input.as_ref()?.poll()
    }

    /// The hotkey came up: the take is transcribed.
    fn finish(&mut self) {
        if let Some(input) = self.input.as_ref() {
            input.end();
        }
    }

    /// Escape, or the end of the window: the take is dropped.
    pub(in crate::app) fn cancel(&mut self) {
        if let Some(input) = self.input.as_ref() {
            input.cancel();
        }
        self.try_started = None;
        self.phase = Phase::Idle;
        self.target = None;
        self.started = None;
    }

    /// One sentence of a hotkey that started nothing.
    fn fail(&mut self, message: &str) {
        self.phase = Phase::Idle;
        self.error = Some((message.to_owned(), Instant::now() + ERROR_MARK));
    }

    /// Run one two-second take for the **Try** button of the Settings tab.
    fn try_take(&mut self, ctx: &Context, plan: TakePlan, home: &Path) {
        self.try_answer = None;
        if self.begin(ctx, plan, SETTINGS_TRY, home) {
            self.try_started = Some(Instant::now());
        }
    }

    /// End the take of the **Try** button when its two seconds passed.
    fn poll_try(&mut self, ctx: &Context, now: Instant) {
        let Some(started) = self.try_started else {
            return;
        };
        if now.saturating_duration_since(started) >= TRY_TAKE && self.phase == Phase::Listening {
            self.finish();
            self.try_started = None;
        }
        // The take ends at its two seconds and asks the worker for its answer; keep
        // the frames coming until that answer is in the tab.
        ctx.request_repaint_after(Duration::from_millis(50));
    }

    /// Apply one event of the worker. Returns the text of a take that a panel
    /// receives.
    fn apply(&mut self, event: horizon_core::speech::SpeechEvent) -> Option<(PanelId, String)> {
        let is_try = event.target == SETTINGS_TRY;
        match event.kind {
            SpeechEventKind::Listening => {
                self.phase = Phase::Listening;
            }
            SpeechEventKind::Writing => {
                self.phase = Phase::Writing;
            }
            SpeechEventKind::Text(text) => {
                self.phase = Phase::Idle;
                self.started = None;
                if is_try {
                    self.try_answer = Some(Ok(text));
                    return None;
                }
                return Some((event.target, text));
            }
            SpeechEventKind::Failed(message) => {
                self.phase = Phase::Idle;
                self.started = None;
                if is_try {
                    self.try_answer = Some(Err(message));
                } else {
                    self.fail(&message);
                }
            }
            SpeechEventKind::Dropped => {
                self.phase = Phase::Idle;
                self.started = None;
                if is_try {
                    self.try_answer = Some(Err("The take held no text".to_owned()));
                }
            }
        }
        None
    }
}

/// `elapsed` as the minutes and seconds of the mark, such as `0:03`.
fn clock_text(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

impl HorizonApp {
    /// The hotkey of speech input: a press records, a release transcribes, and
    /// Escape drops the take.
    ///
    /// This runs before the filter of the held keys, so the release of the hotkey
    /// still reaches the take. The press is taken out of the input of the terminal,
    /// so the key never reaches the program of the panel.
    pub(super) fn handle_speech_key(&mut self, ctx: &Context) {
        let Some(binding) = self.speech.binding else {
            return;
        };
        let (start, end, cancel) = ctx.input(|input| {
            // A repeat of a key that is down is not a second press.
            let start = shortcut_pressed_in_events(&input.events, binding) && self.speech.is_idle();
            (
                start,
                shortcut_released_in_events(&input.events, binding),
                key_pressed_in_events(&input.events, Key::Escape),
            )
        });
        if cancel && self.speech.is_listening() {
            self.speech.cancel();
            self.consume_navigation_key(ctx, ShortcutBinding::new(ShortcutModifiers::NONE, ShortcutKey::Escape));
            return;
        }
        if end && self.speech.is_listening() {
            self.speech.finish();
            return;
        }
        if !start {
            return;
        }
        // The press belongs to the take, whatever the take does with it.
        self.consume_navigation_key(ctx, binding);
        let Some(plan) = self.template_config.speech.plan() else {
            self.speech.fail(NOT_SET);
            return;
        };
        let Some(target) = self.speech_target() else {
            self.speech.fail(NO_TERMINAL);
            return;
        };
        self.speech.begin(ctx, plan, target, self.session_store.home().root());
    }

    /// The hotkey of the only section that speech input reads.
    pub(super) fn apply_speech_config(&mut self, config: &horizon_core::SpeechConfig) {
        let mut whole = self.template_config.clone();
        whole.speech = config.clone();
        self.speech.configure(&whole, self.named_secret_vault.clone());
    }

    /// The panel that a take records for: the focused panel, when it is one that a
    /// paste enters.
    fn speech_target(&self) -> Option<PanelId> {
        let focused = self.board.focused?;
        self.board
            .panel(focused)
            .filter(|panel| panel.visible && panel.terminal().is_some())
            .map(|panel| panel.id)
    }

    /// Apply the answers of the worker of speech input, and keep the mark fresh.
    pub(super) fn poll_speech(&mut self, ctx: &Context) {
        while let Some(event) = self.speech.poll() {
            let Some((target, text)) = self.speech.apply(event) else {
                continue;
            };
            // The text is untrusted, so the paste path of the terminal cleans it
            // again and wraps it in one bracketed paste when the program asked for
            // one. No line break follows it: the person decides when to send it.
            if !self.paste_native_text_into_terminal(target, &text, false) {
                self.speech.fail("The panel of the take is gone.");
            }
        }
        let now = Instant::now();
        self.speech.poll_try(ctx, now);
        if self.speech.phase == Phase::Listening {
            // The seconds of the mark are one repaint per second, and only while a
            // take records.
            ctx.request_repaint_after(LISTENING_TICK);
        } else if let Some((_, until)) = &self.speech.error
            && *until > now
        {
            // One frame at the end of the sentence of a failed take.
            ctx.request_repaint_after(until.saturating_duration_since(now));
        }
    }

    /// The **Try** button of the Settings tab.
    pub(super) fn try_speech(&mut self, ctx: &Context) {
        let Some(plan) = self.template_config.speech.plan() else {
            self.speech.try_answer = Some(Err(NOT_SET.to_owned()));
            return;
        };
        self.speech.try_take(ctx, plan, self.session_store.home().root());
    }

    /// Draw the state mark of a take on the panel of that take, inside `rect`.
    pub(super) fn paint_speech_mark(&self, painter: &egui::Painter, panel: PanelId, rect: egui::Rect) {
        if self.speech.target() != Some(panel) {
            return;
        }
        let Some((text, tone)) = self.speech.mark(Instant::now()) else {
            return;
        };
        pill::paint(painter, rect, &text, tone);
    }
}

/// The color of one tone of the mark: the accent of a recording, the soft text of
/// a transcription, and the red of a sentence.
pub(super) fn tone_color(tone: MarkTone) -> egui::Color32 {
    match tone {
        MarkTone::Listening => theme::ACCENT(),
        MarkTone::Writing => theme::FG_DIM(),
        MarkTone::Error => theme::PALETTE_RED(),
    }
}

#[cfg(test)]
mod tests;

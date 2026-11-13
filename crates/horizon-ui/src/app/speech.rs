//! The hotkey, the insert, the microphone icon and the sentence of speech input.
//!
//! The window sends one key event to the worker thread of
//! [`horizon_core::speech`] and reads its events: the recorder runs while the
//! hotkey is down, and the text of the take goes into the panel that had the focus
//! when the key went down, through the paste path of the terminal.
//!
//! The icon of the title bar of a panel states what the take of that panel does, and
//! one press on it starts or stops that take. The sentence of a failed take or of a
//! kept take is a small pill of the panel of the take.

mod pill;

pub(in crate::app) use pill::MarkTone;

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

/// The window that reads the hotkey of speech input, and holds the panel of its
/// take.
///
/// A window of Horizon draws one workspace of the board, so the panel of a take is
/// the focused panel of the window that the hotkey came from. The main window holds
/// every workspace that is not detached.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::app) enum SpeechWindow {
    Root,
    Detached(horizon_core::WorkspaceId),
}

impl SpeechWindow {
    /// Whether `workspace` is drawn by this window.
    fn holds(self, detached: bool, workspace: horizon_core::WorkspaceId) -> bool {
        match self {
            Self::Root => !detached,
            Self::Detached(detached_workspace) => detached && detached_workspace == workspace,
        }
    }
}

/// How long the sentence of a failed take stays on the panel.
const ERROR_MARK: Duration = Duration::from_secs(6);

/// How long the sentence of a kept take stays on the panel. It stays longer than
/// another failure, because it offers the one retry of the take.
const KEPT_MARK: Duration = Duration::from_secs(30);

/// The shortcut that sends the last kept take again: the primary modifier, Shift
/// and `R`. It is fixed, because it only acts while a kept take waits, and the
/// Settings tab names it.
const RETRY_SHORTCUT: ShortcutBinding =
    ShortcutBinding::new(ShortcutModifiers::PRIMARY_SHIFT, ShortcutKey::Letter('R'));

/// Why the sentence of this window shows a take: the hotkey, the icon of a panel, or
/// the **Try** button of the Settings tab.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TakeSource {
    Hotkey,
    Icon,
    Settings,
}

/// The state that the microphone icon of a panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum MicState {
    /// No take records for this panel.
    Idle,
    /// The recorder of the take of this panel runs.
    Recording,
    /// The take of this panel is transcribed.
    Busy,
}

/// The microphone icon of the title bar of one panel: the state that it shows and
/// what a press on it does.
#[derive(Clone, Copy)]
pub(in crate::app) struct PanelMic {
    pub state: MicState,
    /// Whether a press does anything: no other take runs, the window answers, and
    /// the panel can take a take.
    pub enabled: bool,
    /// Whether a take of this window runs at all, so the icon of another panel says
    /// that the take is elsewhere.
    pub active: bool,
    /// When the take of this panel started to record, while it records: the hover
    /// text and the value of the control show its clock.
    pub started: Option<Instant>,
    /// The hotkey of the settings, while they name one.
    pub hotkey: Option<ShortcutBinding>,
}

/// How long the take of the **Try** button of the Settings tab records.
const TRY_TAKE: Duration = Duration::from_secs(2);

/// The sentence of a hotkey that no terminal can receive.
pub(super) const NO_TERMINAL: &str = "Click a terminal first.";

/// The sentence of a hotkey while the settings name no recorder or no transcriber.
pub(super) const NOT_SET: &str = "Speech input is off. Set a recorder and a transcriber in Settings.";

/// The panel of a take that the Settings tab runs, so no panel receives its text.
pub(super) const SETTINGS_TRY: PanelId = PanelId(u64::MAX);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// The hotkey of the settings, or `None` while the settings name none.
    binding: Option<ShortcutBinding>,
    phase: Phase,
    /// The control that started the take that runs, so only the take of the hotkey
    /// ends when its key comes up.
    source: Option<TakeSource>,
    /// The panel that had the focus when the hotkey went down.
    target: Option<PanelId>,
    /// When the take started to record, for the seconds of the mark.
    started: Option<Instant>,
    /// The sentence of a failed take and the instant that it goes.
    error: Option<(String, Instant)>,
    /// The sentence of a take that could not be sent and stays for one retry,
    /// while that file waits in the folder of the takes.
    kept: Option<String>,
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
            binding: None,
            phase: Phase::Idle,
            source: None,
            target: None,
            started: None,
            error: None,
            kept: None,
            try_started: None,
            try_answer: None,
            asked_key: None,
        }
    }

    /// The hotkey of `config`.
    pub(in crate::app) fn configure(&mut self, config: &horizon_core::Config) {
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

    /// Whether a take of this window runs at all, whatever panel it belongs to.
    pub(in crate::app) fn is_active(&self) -> bool {
        !self.is_idle()
    }

    /// Whether the release of the hotkey ends the take that runs: only a take that
    /// the hotkey started ends with the key, because a press on the icon is its own
    /// start and its own stop.
    pub(in crate::app) fn ends_with_hotkey_release(&self) -> bool {
        self.is_listening() && self.source == Some(TakeSource::Hotkey)
    }

    /// The state that the icon of `panel` shows, from the phases of the take.
    pub(in crate::app) fn mic_state(&self, panel: PanelId) -> MicState {
        if self.target != Some(panel) {
            return MicState::Idle;
        }
        match self.phase {
            Phase::Idle => MicState::Idle,
            Phase::Listening => MicState::Recording,
            Phase::Writing => MicState::Busy,
        }
    }

    /// When the take that records started, for the clock of its icon.
    pub(in crate::app) fn take_started(&self) -> Option<Instant> {
        self.is_listening().then_some(self.started).flatten()
    }

    /// The hotkey of the settings, while they name one.
    pub(in crate::app) fn hotkey(&self) -> Option<ShortcutBinding> {
        self.binding
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

    /// The sentence of the take that waits for its one retry, when there is one.
    pub(in crate::app) fn kept_take(&self) -> Option<&str> {
        self.kept.as_deref()
    }

    /// Send the last kept take again. Returns whether a retry started.
    fn retry(&mut self, ctx: &Context, plan: TakePlan, home: &Path, keys: Option<Arc<NamedSecretVault>>) -> bool {
        // A retry is not a press of the hotkey or of an icon: nothing but its own
        // answer ends it.
        if !self.is_idle() || self.kept.is_none() || self.target.is_none() {
            return false;
        }
        let Some(target) = self.target else {
            return false;
        };
        self.begin_input(ctx, home, keys);
        let Some(input) = self.input.as_ref() else {
            return false;
        };
        input.retry(plan, target);
        // One retry of the last take: the offer goes with it, and the next answer
        // either inserts the text or offers the retry that its own failure earns.
        self.kept = None;
        self.error = None;
        self.phase = Phase::Writing;
        self.started = Some(Instant::now());
        true
    }

    /// The answer of the **Try** button of the Settings tab, and whether it waits.
    pub(in crate::app) fn try_answer(&self) -> Option<&Result<String, String>> {
        self.try_answer.as_ref()
    }

    pub(in crate::app) fn try_running(&self) -> bool {
        self.try_started.is_some()
    }

    /// Start one take of `plan` into `target`.
    fn begin(
        &mut self,
        ctx: &Context,
        plan: TakePlan,
        target: PanelId,
        source: TakeSource,
        home: &Path,
        keys: Option<Arc<NamedSecretVault>>,
    ) -> bool {
        if !self.is_idle() {
            return false;
        }
        self.begin_input(ctx, home, keys);
        let Some(input) = self.input.as_ref() else {
            return false;
        };
        input.begin(plan, target);
        self.phase = Phase::Listening;
        self.source = Some(source);
        self.target = Some(target);
        self.started = Some(Instant::now());
        self.error = None;
        true
    }

    /// Start the worker of this window once, with the home and the secrets of its
    /// first take.
    fn begin_input(&mut self, ctx: &Context, home: &Path, keys: Option<Arc<NamedSecretVault>>) {
        let home = self.home.get_or_insert_with(|| home.to_path_buf()).clone();
        let wake_context = ctx.clone();
        let keys = keys_reader(keys);
        self.input.get_or_insert_with(move || {
            SpeechInput::start(
                SpeechWorker::new(home, keys).with_wake(Arc::new(move || wake_context.request_repaint())),
            )
        });
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
        self.source = None;
        self.target = None;
        self.started = None;
    }

    /// One sentence of a hotkey that started nothing.
    fn fail(&mut self, message: &str) {
        self.phase = Phase::Idle;
        self.kept = None;
        self.error = Some((message.to_owned(), Instant::now() + ERROR_MARK));
    }

    /// One sentence of a take that stays for one retry. The mark names the
    /// shortcut of that retry, and the Settings tab offers the same with a button.
    fn fail_kept(&mut self, message: &str) {
        self.phase = Phase::Idle;
        let shortcut = retry_shortcut_label();
        self.kept = Some(format!(
            "The take is kept and can be sent again. {shortcut} sends it again."
        ));
        self.error = Some((
            format!("{message} The take is kept: {shortcut} sends it again."),
            Instant::now() + KEPT_MARK,
        ));
    }

    /// Run one two-second take for the **Try** button of the Settings tab.
    fn try_take(&mut self, ctx: &Context, plan: TakePlan, home: &Path, keys: Option<Arc<NamedSecretVault>>) {
        self.try_answer = None;
        if self.begin(ctx, plan, SETTINGS_TRY, TakeSource::Settings, home, keys) {
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
            SpeechEventKind::Kept(message) => {
                self.phase = Phase::Idle;
                self.started = None;
                if is_try {
                    self.try_answer = Some(Err(message));
                } else {
                    self.fail_kept(&message);
                }
            }
            SpeechEventKind::Failed(message) => {
                self.phase = Phase::Idle;
                self.started = None;
                self.kept = None;
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

/// The reader of the named secrets of this computer, or one that holds no value
/// while this window has no secrets file.
fn keys_reader(keys: Option<Arc<NamedSecretVault>>) -> Arc<dyn horizon_core::speech::SpeechKeyStore> {
    match keys {
        Some(keys) => keys,
        None => Arc::new(()),
    }
}

/// The name of the primary modifier of this platform, as the Settings tab and the
/// icon show it in a shortcut.
pub(in crate::app) fn primary_modifier_label() -> &'static str {
    if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" }
}

/// The shortcut that sends a kept take again, as a person reads it. The pill and
/// the Settings tab name the same keys.
pub(in crate::app) fn retry_shortcut_label() -> String {
    RETRY_SHORTCUT.display_label(primary_modifier_label())
}

/// `elapsed` as the minutes and seconds of a take, such as `0:03`.
pub(in crate::app) fn clock_text(elapsed: Duration) -> String {
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
    pub(super) fn handle_speech_key(&mut self, ctx: &Context, window: SpeechWindow) {
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
        if ctx.input(|input| shortcut_pressed_in_events(&input.events, RETRY_SHORTCUT))
            && self.speech.kept_take().is_some()
        {
            // The kept take waits for one retry, and nothing else takes this
            // shortcut while it does.
            self.consume_navigation_key(ctx, RETRY_SHORTCUT);
            self.retry_speech_take(ctx);
            return;
        }
        if start {
            // The press belongs to the take, whatever the take does with it.
            self.consume_navigation_key(ctx, binding);
            self.start_speech_take(ctx, window);
        }
        // A tap whose release is in the same frame ends the take of that frame: a
        // take of a few milliseconds is dropped, as every short take is. A take that
        // the icon started has no key to come up, so this release never ends it.
        if end && self.speech.ends_with_hotkey_release() {
            self.speech.finish();
        }
    }

    /// Start the take of one press of the hotkey, or say why nothing started.
    fn start_speech_take(&mut self, ctx: &Context, window: SpeechWindow) {
        let target = self.speech_target(window);
        self.begin_speech_take(ctx, target, TakeSource::Hotkey);
    }

    /// Start one take into `target`, or say why nothing started.
    fn begin_speech_take(&mut self, ctx: &Context, target: Option<PanelId>, source: TakeSource) {
        let Some(plan) = self.template_config.speech.plan() else {
            self.speech.fail(NOT_SET);
            return;
        };
        let Some(target) = target else {
            self.speech.fail(NO_TERMINAL);
            return;
        };
        let home = self.session_store.home().root().to_path_buf();
        let keys = self.named_secret_vault.clone();
        self.speech.begin(ctx, plan, target, source, &home, keys);
    }

    /// Whether a take of speech input can go into `panel` while `window` draws it.
    ///
    /// This is the one rule of the target of a take: the hotkey takes it, the icon of
    /// the title bar is drawn for it, and a press on the icon starts it. A panel that
    /// another window draws is no target, because the person does not see it here.
    fn speech_panel_accepts(&self, panel_id: PanelId, window: SpeechWindow) -> bool {
        self.board.panel(panel_id).is_some_and(|panel| {
            panel.visible
                && panel.terminal().is_some()
                && window.holds(self.workspace_is_detached(panel.workspace_id), panel.workspace_id)
        })
    }

    /// The hotkey of the only section that speech input reads.
    pub(super) fn apply_speech_config(&mut self, config: &horizon_core::SpeechConfig) {
        let mut whole = self.template_config.clone();
        whole.speech = config.clone();
        self.speech.configure(&whole);
    }

    /// The panel that a take records for, in the window that read the hotkey: the
    /// focused panel, when this window draws it and a paste enters it.
    fn speech_target(&self, window: SpeechWindow) -> Option<PanelId> {
        let focused = self.board.focused?;
        self.speech_panel_accepts(focused, window).then_some(focused)
    }

    /// The microphone icon of the title bar of `panel`, or `None` while that panel
    /// shows no icon.
    ///
    /// The icon is drawn only while speech input is on, and only on a panel that a
    /// take could enter: the rule of [`Self::speech_panel_accepts`] is the whole
    /// rule of both.
    pub(in crate::app) fn panel_mic(
        &self,
        panel_id: PanelId,
        window: SpeechWindow,
        interactive: bool,
    ) -> Option<PanelMic> {
        if !self.template_config.speech.is_on() || !self.speech_panel_accepts(panel_id, window) {
            return None;
        }
        let state = self.speech.mic_state(panel_id);
        Some(PanelMic {
            state,
            enabled: panel_mic_enabled(interactive, self.speech.is_active(), state),
            active: self.speech.is_active(),
            started: self.speech.take_started(),
            hotkey: self.speech.hotkey(),
        })
    }

    /// One press on the microphone icon of `panel`: the start of a take into that
    /// panel, or the end of the take of that panel.
    pub(super) fn click_speech_mic(&mut self, ctx: &Context, panel_id: PanelId, window: SpeechWindow) {
        if self.speech.target() == Some(panel_id) && self.speech.is_listening() {
            self.speech.finish();
            // The pulse of the icon and the answer of the worker need frames of
            // their own.
            ctx.request_repaint();
            return;
        }
        // The icon of a panel is disabled while a take of another panel runs, and
        // the icon of a transcribed take takes no press.
        if !self.speech.is_idle() {
            return;
        }
        let target = self.speech_panel_accepts(panel_id, window).then_some(panel_id);
        self.begin_speech_take(ctx, target, TakeSource::Icon);
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
        if self.speech.is_active() {
            // The pulse of the icon of the take needs every frame, and only while its
            // icon is red or amber. A window whose every icon is idle asks for no
            // frame of its own.
            ctx.request_repaint();
        } else if let Some((_, until)) = &self.speech.error
            && *until > now
        {
            // One frame at the end of the sentence of a failed take.
            ctx.request_repaint_after(until.saturating_duration_since(now));
        }
    }

    /// Send the last kept take again, from the shortcut or the Settings tab.
    pub(super) fn retry_speech_take(&mut self, ctx: &Context) {
        let Some(plan) = self.template_config.speech.plan() else {
            self.speech.fail(NOT_SET);
            return;
        };
        let home = self.session_store.home().root().to_path_buf();
        let keys = self.named_secret_vault.clone();
        if !self.speech.retry(ctx, plan, &home, keys) {
            self.speech.fail("There is no kept take to send again.");
        }
    }

    /// The **Try** button of the Settings tab.
    pub(super) fn try_speech(&mut self, ctx: &Context) {
        let Some(plan) = self.template_config.speech.plan() else {
            self.speech.try_answer = Some(Err(NOT_SET.to_owned()));
            return;
        };
        self.speech.try_take(
            ctx,
            plan,
            self.session_store.home().root(),
            self.named_secret_vault.clone(),
        );
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

/// Whether a press on the icon does anything: the window answers, and only the icon
/// of the take that runs can end it.
pub(in crate::app) const fn panel_mic_enabled(interactive: bool, speech_active: bool, state: MicState) -> bool {
    interactive
        && match state {
            MicState::Idle => !speech_active,
            MicState::Recording => true,
            MicState::Busy => false,
        }
}

#[cfg(test)]
mod tests;

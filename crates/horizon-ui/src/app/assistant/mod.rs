//! The assistant drawer: a docked right-hand panel hosting one agent terminal.
//!
//! The agent is an ordinary agent panel marked by a well-known local id. It is
//! created hidden, so it never appears on the canvas, and the drawer draws its
//! terminal. Closing the drawer leaves the agent running.

mod blocks;
mod cards;
mod command_bar;
mod drawer;
mod engine;
mod icons;
mod num;
mod plan;
mod reach;
mod scope;
mod summon;
mod threads;

use egui::{Context, Id, Pos2, Rect};
use egui_commonmark::CommonMarkCache;
use horizon_core::assistant::{ASSISTANT_PANEL_LOCAL_ID, AssistantSettings, Thread, Threads};
use horizon_core::browser::manifest::agent_panels::{AgentPanel, PlanStep};
use horizon_core::{HorizonHome, PanelId, PanelOptions, PanelResume};
use zeroize::Zeroizing;

use super::{HorizonApp, TOOLBAR_HEIGHT};

pub(super) const ASSISTANT_PANEL_ID: &str = "assistant_drawer";
const DEFAULT_WIDTH: f32 = 460.0;
const MIN_WIDTH: f32 = 340.0;
const START_RETRY: std::time::Duration = std::time::Duration::from_secs(5);

pub(super) struct AssistantDrawer {
    open: bool,
    /// The drawer terminal owns the keyboard.
    focused: bool,
    /// Canvas focus to hand back when the drawer lets go of the keyboard.
    previous_focus: Option<PanelId>,
    /// The engine the running agent was started with.
    settings: AssistantSettings,
    /// Selection staged in the engine popup, applied on restart.
    draft: AssistantSettings,
    /// Close the running agent at the start of the next drawer render.
    restart_requested: bool,
    engine_open: bool,
    engine_anchor: Option<Rect>,
    cards: cards::Cards,
    /// The conversation as the desk-mode feed shows it.
    feed: summon::feed::Feed,
    command: command_bar::CommandBar,
    summon: summon::Summon,
    /// Which workspaces the assistant is looking at.
    scope: scope::Scope,
    scope_open: bool,
    scope_anchor: Option<Rect>,
    /// Desktop-workspace mode, when switched on.
    desk: Option<crate::app::desk::Desk>,
    /// Scripted input for demos, when asked for.
    demo: Option<summon::demo::Demo>,
    /// The assistant's own report of the steps of what was asked, if it sent one.
    plan: Vec<PlanStep>,
    md_cache: CommonMarkCache,
    /// Where the assistant keeps its settings, key and threads.
    home: HorizonHome,
    threads: Threads,
    /// The session the running agent is in, once it has one.
    active_session: Option<String>,
    last_thread_sync: Option<std::time::Instant>,
    /// A thread to resume the next time the agent starts.
    resume: Option<Thread>,
    thread_menu_open: bool,
    thread_anchor: Option<Rect>,
    key_input: Zeroizing<String>,
    notice: Option<String>,
    /// Do not try to start the agent again before this, after a failed start.
    retry_at: Option<std::time::Instant>,
    reach_cache: Option<(std::time::Instant, Vec<AgentPanel>)>,
}

impl AssistantDrawer {
    pub(super) fn new(home: &HorizonHome) -> Self {
        let settings = AssistantSettings::load(home);
        Self {
            open: false,
            focused: false,
            previous_focus: None,
            settings,
            draft: settings,
            restart_requested: false,
            engine_open: false,
            engine_anchor: None,
            cards: cards::Cards::default(),
            feed: summon::feed::Feed::default(),
            command: command_bar::CommandBar::default(),
            summon: summon::Summon::default(),
            scope: scope::Scope::default(),
            scope_open: false,
            scope_anchor: None,
            desk: crate::app::desk::Desk::from_env(),
            demo: summon::demo::Demo::from_env(),
            plan: Vec::new(),
            md_cache: CommonMarkCache::default(),
            home: home.clone(),
            threads: Threads::load(home),
            active_session: None,
            last_thread_sync: None,
            resume: None,
            thread_menu_open: false,
            thread_anchor: None,
            key_input: Zeroizing::new(String::new()),
            notice: None,
            retry_at: None,
            reach_cache: None,
        }
    }

    /// Forgets everything tied to the previous board: its cards, focus and
    /// session point at panels that no longer exist.
    pub(super) fn reset_for_new_board(&mut self) {
        self.cards.clear();
        self.feed.clear();
        self.command = command_bar::CommandBar::default();
        self.summon = summon::Summon::default();
        self.plan.clear();
        self.focused = false;
        self.previous_focus = None;
        self.active_session = None;
        self.resume = None;
        self.restart_requested = false;
        self.retry_at = None;
        self.reach_cache = None;
    }
}

impl HorizonApp {
    /// Opens the drawer and gives it the keyboard if it is not already open.
    pub(in crate::app) fn open_assistant(&mut self) {
        if !self.assistant.open {
            self.toggle_assistant();
        }
    }

    /// Opens the drawer and gives it the keyboard, or closes it.
    pub(super) fn toggle_assistant(&mut self) {
        if self.assistant.open {
            self.assistant.open = false;
            self.assistant.engine_open = false;
            self.release_assistant_focus();
        } else {
            self.assistant.open = true;
            self.focus_assistant();
        }
    }

    /// Whether the drawer is on screen. Settings and fullscreen views take the whole window.
    pub(super) fn assistant_visible(&self) -> bool {
        self.assistant.open && self.settings.is_none() && self.fullscreen_panel.is_none()
    }

    pub(super) fn assistant_panel_rect(&self, ctx: &Context, viewport: Rect) -> Option<Rect> {
        if !self.assistant_visible() {
            return None;
        }
        let remembered =
            egui::containers::panel::PanelState::load(ctx, Id::new(ASSISTANT_PANEL_ID)).map(|state| state.outer_rect);
        remembered.or_else(|| {
            let width = DEFAULT_WIDTH.min(viewport.width() * 0.6).max(MIN_WIDTH);
            Some(Rect::from_min_max(
                Pos2::new(viewport.max.x - width, viewport.min.y + TOOLBAR_HEIGHT),
                viewport.max,
            ))
        })
    }

    pub(in crate::app) fn assistant_has_keyboard(&self) -> bool {
        self.assistant.focused
    }

    /// Width the drawer takes at the window's right edge, for overlays anchored there.
    pub(in crate::app) fn assistant_right_inset(&self, ctx: &Context) -> f32 {
        let viewport = crate::app::util::viewport_local_rect(ctx);
        self.assistant_panel_rect(ctx, viewport)
            .map_or(0.0, |rect| (viewport.max.x - rect.min.x).max(0.0))
    }

    fn focus_assistant(&mut self) {
        if self.assistant.focused {
            return;
        }
        self.assistant.focused = true;
        self.assistant.previous_focus = self.board.focused.take();
    }

    fn release_assistant_focus(&mut self) {
        self.assistant.focused = false;
        if self.board.focused.is_none() {
            self.board.focused = self
                .assistant
                .previous_focus
                .take()
                .filter(|id| self.board.panel(*id).is_some_and(|panel| panel.visible));
        }
    }

    /// A click on a canvas panel moves focus there; the drawer then stops typing.
    fn sync_assistant_focus(&mut self) {
        if self.assistant.focused && self.board.focused.is_some() {
            self.assistant.focused = false;
            self.assistant.previous_focus = None;
        }
    }

    /// Starts the agent the first time the drawer needs it.
    fn ensure_assistant_panel(&mut self, ctx: &Context) {
        if self.board.assistant_panel().is_some() {
            return;
        }
        if let Err(reason) = self.assistant.settings.launch_readiness(&self.assistant.home) {
            self.assistant.notice = Some(reason);
            return;
        }
        let workspace_id = self
            .board
            .active_workspace
            .unwrap_or_else(|| self.ensure_workspace_visible(ctx));
        if self.assistant.retry_at.is_some_and(|at| std::time::Instant::now() < at) {
            return;
        }
        let resume = self.assistant.resume.clone();
        let options = PanelOptions {
            kind: self.assistant.settings.agent,
            name: Some("Assistant".to_string()),
            name_is_custom: Some(true),
            local_id: Some(ASSISTANT_PANEL_LOCAL_ID.to_string()),
            visible: false,
            resume: resume
                .as_ref()
                .map_or(PanelResume::Fresh, |thread| PanelResume::Session {
                    session_id: thread.session_id.clone(),
                }),
            cwd: resume
                .as_ref()
                .and_then(|thread| thread.cwd.as_deref())
                .map(std::path::PathBuf::from),
            ..PanelOptions::default()
        };
        match self.create_panel_with_options(options, workspace_id) {
            Ok(_) => {
                self.assistant.notice = None;
                self.assistant.resume = None;
                self.assistant.retry_at = None;
            }
            Err(error) => {
                self.assistant.retry_at = Some(std::time::Instant::now() + START_RETRY);
                tracing::error!("failed to start the assistant: {error}");
                self.assistant.notice = Some(format!("Could not start the assistant: {error}"));
            }
        }
    }

    /// Ends the running agent so the drawer starts a fresh one with the current settings.
    fn restart_assistant(&mut self) {
        self.assistant.restart_requested = true;
        self.assistant.notice = None;
    }

    /// Closes the agent directly: the shared close queue is rebuilt by the canvas
    /// every frame, so a request queued from the drawer would be dropped.
    fn close_assistant_if_restarting(&mut self) {
        if !std::mem::take(&mut self.assistant.restart_requested) {
            return;
        }
        if let Some(panel_id) = self.board.assistant_panel() {
            self.close_panel(panel_id);
            self.panel_screen_rects.remove(&panel_id);
            self.terminal_body_screen_rects.remove(&panel_id);
        }
    }

    /// Applies the engine chosen in the popup, restarting the agent when it changed.
    fn apply_assistant_engine(&mut self) {
        let draft = self.assistant.draft;
        if draft.same_engine(&self.assistant.settings) {
            return;
        }
        if let Err(error) = draft.save(&self.assistant.home) {
            self.assistant.notice = Some(format!("Could not save the assistant settings: {error}"));
            return;
        }
        self.assistant.settings = draft;
        self.restart_assistant();
    }
}

#[cfg(test)]
mod tests;

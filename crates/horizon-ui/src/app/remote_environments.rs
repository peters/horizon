//! Lazy, single-flight saved inventory loading for the Remote Environments overview.

mod observation;
mod paint;
mod reconnect;
mod stop;

use std::sync::mpsc::{self, Receiver, TryRecvError};

use egui::Context;
use horizon_core::{HorizonHome, cloud_run::CloudWorkflowStore, remote_workspace::RemoteEnvironmentSummary};

use super::HorizonApp;

#[derive(Default)]
pub(super) struct RemoteEnvironments {
    open: bool,
    page: Option<InventoryPage>,
    page_cursor: Option<String>,
    selected: Option<usize>,
    failure: Option<LoadFailure>,
    pending: Option<PendingLoad>,
    refresh_when_idle: bool,
    observation: observation::ObservationState,
    stop: stop::StopState,
    reconnect: reconnect::ReconnectState,
    refresh_after_stop: bool,
}

struct InventoryPage {
    rows: Vec<paint::InventoryRow>,
    next_cursor: Option<String>,
}

struct PendingLoad {
    rx: Receiver<Result<InventoryPage, LoadError>>,
    cursor: Option<String>,
    discard: bool,
}

struct LoadFailure {
    error: LoadError,
    cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoadError {
    OpenStore,
    ReadPage,
    WorkerUnavailable,
}

#[derive(Clone, Copy, Default)]
enum InventoryAction {
    #[default]
    None,
    Close,
    Refresh,
    First,
    Next,
    Retry,
    Select(usize),
    Observe,
    RequestStop,
    ConfirmStop,
    CancelStop,
    ListReconnectViews,
    Reconnect(horizon_core::PanelId),
}

struct WakeOnDrop(Context);

impl Drop for WakeOnDrop {
    fn drop(&mut self) {
        self.0.request_repaint();
    }
}

impl RemoteEnvironments {
    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn invalidate_session_views(&mut self) {
        self.reconnect.invalidate();
    }

    pub(super) fn open(&mut self, home: &HorizonHome, ctx: &Context) {
        if self.open {
            return;
        }
        self.open = true;
        self.reconnect.invalidate();
        self.page = None;
        self.page_cursor = None;
        self.selected = None;
        self.failure = None;
        self.observation.invalidate();
        self.stop.invalidate();
        if let Some(pending) = &mut self.pending {
            pending.discard = true;
            self.refresh_when_idle = true;
        } else {
            self.start_load(home, ctx, None);
        }
        ctx.request_repaint();
    }

    fn close(&mut self) {
        self.open = false;
        self.reconnect.invalidate();
        self.observation.invalidate();
        self.stop.invalidate();
        self.refresh_after_stop = false;
        self.refresh_when_idle = false;
        if let Some(pending) = &mut self.pending {
            pending.discard = true;
        }
    }

    fn start_load(&mut self, home: &HorizonHome, ctx: &Context, cursor: Option<String>) {
        if !self.open || self.pending.is_some() {
            return;
        }
        self.reconnect.invalidate();
        self.observation.invalidate();
        self.stop.cancel_confirmation();
        let (tx, rx) = mpsc::sync_channel(1);
        let home = home.clone();
        let requested_cursor = cursor.clone();
        let wake = WakeOnDrop(ctx.clone());
        let worker = std::thread::Builder::new()
            .name("remote-environment-inventory".into())
            .spawn(move || {
                let _wake = wake;
                let result = load_page(&home, requested_cursor.as_deref());
                let _ = tx.send(result);
            });
        self.failure = None;
        match worker {
            Ok(_) => {
                self.pending = Some(PendingLoad {
                    rx,
                    cursor,
                    discard: false,
                });
            }
            Err(_) => self.accept_result(cursor, Err(LoadError::WorkerUnavailable)),
        }
    }

    fn drain_result(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let result = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.pending = Some(pending);
                return;
            }
            Err(TryRecvError::Disconnected) => Err(LoadError::WorkerUnavailable),
        };
        if self.open && !pending.discard {
            self.accept_result(pending.cursor, result);
        }
    }

    fn accept_result(&mut self, cursor: Option<String>, result: Result<InventoryPage, LoadError>) {
        self.reconnect.invalidate();
        self.observation.invalidate();
        self.stop.cancel_confirmation();
        match result {
            Ok(page) => {
                let previous = self
                    .page
                    .as_ref()
                    .and_then(|page| self.selected.and_then(|index| page.rows.get(index)));
                self.selected = previous
                    .and_then(|previous| {
                        page.rows
                            .iter()
                            .position(|row| row.summary.workspace_local_id == previous.summary.workspace_local_id)
                    })
                    .or_else(|| (!page.rows.is_empty()).then_some(0));
                self.page = Some(page);
                self.page_cursor = cursor;
                self.failure = None;
            }
            Err(error) => self.failure = Some(LoadFailure { error, cursor }),
        }
    }

    fn apply(&mut self, action: InventoryAction, home: &HorizonHome, ctx: &Context) {
        if self.open
            && self.pending.is_none()
            && matches!(
                action,
                InventoryAction::Refresh | InventoryAction::First | InventoryAction::Next | InventoryAction::Retry
            )
        {
            self.stop.invalidate();
        }
        match action {
            InventoryAction::None
            | InventoryAction::Observe
            | InventoryAction::RequestStop
            | InventoryAction::ConfirmStop
            | InventoryAction::ListReconnectViews
            | InventoryAction::Reconnect(_) => {}
            InventoryAction::CancelStop => self.stop.cancel_confirmation(),
            InventoryAction::Close => self.close(),
            InventoryAction::Select(index) => {
                if self.selected != Some(index) && self.page.as_ref().is_some_and(|page| index < page.rows.len()) {
                    self.reconnect.invalidate();
                    self.observation.invalidate();
                    self.stop.invalidate();
                    self.selected = Some(index);
                }
            }
            InventoryAction::Refresh => self.start_load(home, ctx, self.page_cursor.clone()),
            InventoryAction::First => self.start_load(home, ctx, None),
            InventoryAction::Next => {
                if let Some(cursor) = self.page.as_ref().and_then(|page| page.next_cursor.clone()) {
                    self.start_load(home, ctx, Some(cursor));
                }
            }
            InventoryAction::Retry => {
                if let Some(failure) = &self.failure {
                    self.start_load(home, ctx, failure.cursor.clone());
                }
            }
        }
    }

    pub(super) fn invalidate_provider_state(&mut self) {
        self.reconnect.invalidate();
        self.observation.invalidate();
        self.stop.invalidate();
    }

    fn start_observation(
        &mut self,
        home: &HorizonHome,
        config: &horizon_core::remote_provider_config::RemoteProviderConfig,
        ctx: &Context,
    ) {
        if !self.open || self.pending.is_some() || self.stop.is_pending() {
            return;
        }
        if let Some(row) = self
            .page
            .as_ref()
            .and_then(|page| self.selected.and_then(|index| page.rows.get(index)))
        {
            self.observation.start(home, config, &row.summary, ctx);
        }
    }

    fn stop_action(
        &mut self,
        action: InventoryAction,
        home: &HorizonHome,
        config: &horizon_core::remote_provider_config::RemoteProviderConfig,
        ctx: &Context,
    ) {
        if !self.open || self.pending.is_some() || self.observation.is_pending() {
            return;
        }
        let Some(summary) = self
            .page
            .as_ref()
            .and_then(|page| self.selected.and_then(|index| page.rows.get(index)))
            .map(|row| row.summary.clone())
        else {
            return;
        };
        match action {
            InventoryAction::RequestStop => {
                self.reconnect.invalidate();
                self.stop.prepare(&summary, config, ctx);
            }
            InventoryAction::ConfirmStop if self.stop.start(home, config, &summary, ctx) => {
                self.reconnect.invalidate();
                self.observation.invalidate();
            }
            _ => {}
        }
    }

    fn drain_stop(&mut self, home: &HorizonHome, ctx: &Context) {
        if self.stop.drain_result() {
            self.observation.invalidate();
            self.refresh_after_stop = self.open;
        }
        if self.refresh_after_stop && self.pending.is_none() {
            self.refresh_after_stop = false;
            self.start_load(home, ctx, self.page_cursor.clone());
        }
    }
}

fn load_page(home: &HorizonHome, cursor: Option<&str>) -> Result<InventoryPage, LoadError> {
    let store = CloudWorkflowStore::open(home).map_err(|_| LoadError::OpenStore)?;
    let page = store
        .list_remote_environment_page(cursor)
        .map_err(|_| LoadError::ReadPage)?;
    Ok(InventoryPage {
        rows: page
            .records
            .iter()
            .map(|record| paint::InventoryRow::new(record.environment_summary()))
            .collect(),
        next_cursor: page.next_cursor,
    })
}

impl HorizonApp {
    /// Render before root input routing, then consume the entire modal frame's input,
    /// including dismissal. Remote inventory completion never requires idle polling.
    pub(super) fn render_remote_environments(&mut self, ctx: &Context) -> Option<egui::InputState> {
        self.remote_environments.drain_result();
        self.remote_environments.observation.drain_result();
        self.remote_environments.drain_stop(self.session_store.home(), ctx);
        if self.remote_environments.refresh_when_idle && self.remote_environments.pending.is_none() {
            self.remote_environments.refresh_when_idle = false;
            self.remote_environments
                .start_load(self.session_store.home(), ctx, None);
        }
        let was_open = self.remote_environments.open;
        if was_open {
            self.handle_speech_input(ctx);
            let action = paint::show(ctx, &self.remote_environments);
            if matches!(action, InventoryAction::Observe) {
                self.remote_environments.start_observation(
                    self.session_store.home(),
                    &self.template_config.remote,
                    ctx,
                );
            }
            if matches!(action, InventoryAction::RequestStop | InventoryAction::ConfirmStop) {
                self.remote_environments.stop_action(
                    action,
                    self.session_store.home(),
                    &self.template_config.remote,
                    ctx,
                );
            }
            self.remote_environments.apply(action, self.session_store.home(), ctx);
            self.remote_reconnect_action(action, ctx);
            let modal_input = ctx.input(Clone::clone);
            self.suppress_root_viewport_interaction(ctx);
            return Some(modal_input);
        }
        self.remote_reconnect_action(InventoryAction::None, ctx);
        None
    }

    pub(super) fn restore_remote_environment_input(ctx: &Context, saved: egui::InputState) {
        ctx.input_mut(|input| {
            input.pointer = saved.pointer;
            input.keys_down = saved.keys_down;
            input.modifiers = saved.modifiers;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_page(next: Option<&str>) -> InventoryPage {
        InventoryPage {
            rows: Vec::new(),
            next_cursor: next.map(String::from),
        }
    }

    #[test]
    fn failed_next_page_keeps_previous_page_and_exact_retry_cursor() {
        let mut state = RemoteEnvironments {
            open: true,
            ..Default::default()
        };
        state.accept_result(None, Ok(empty_page(Some("page-two"))));
        state.accept_result(Some("page-two".into()), Err(LoadError::ReadPage));
        assert!(state.page_cursor.is_none());
        assert_eq!(
            state.page.as_ref().and_then(|page| page.next_cursor.as_deref()),
            Some("page-two")
        );
        let failure = state.failure.as_ref().expect("failure");
        assert_eq!(failure.cursor.as_deref(), Some("page-two"));
        state.accept_result(Some("page-two".into()), Ok(empty_page(None)));
        assert!(state.failure.is_none());
        assert_eq!(state.page_cursor.as_deref(), Some("page-two"));
        assert!(state.selected.is_none());
    }

    #[test]
    fn repeated_reopen_retains_one_worker_and_discards_its_old_result() {
        let ctx = Context::default();
        let temp = tempfile::tempdir().expect("tempdir");
        let home = HorizonHome::from_root(temp.path().to_path_buf());
        let (tx, rx) = mpsc::sync_channel(1);
        let mut state = RemoteEnvironments {
            open: true,
            pending: Some(PendingLoad {
                rx,
                cursor: Some("old-page".into()),
                discard: false,
            }),
            ..Default::default()
        };
        for _ in 0..100 {
            state.close();
            state.open(&home, &ctx);
            state.start_load(&home, &ctx, None);
            assert!(state.pending.as_ref().is_some_and(|pending| pending.discard));
        }
        tx.send(Ok(empty_page(Some("old-result")))).expect("send");
        state.drain_result();
        assert!(state.page.is_none());
        assert!(state.pending.is_none());
        assert!(state.refresh_when_idle);
        assert!(!home.cloud_workflow_store_path().exists());
    }

    #[test]
    fn closed_view_never_accepts_completion_or_requeues_refresh() {
        let (tx, rx) = mpsc::sync_channel(1);
        let mut state = RemoteEnvironments {
            open: true,
            pending: Some(PendingLoad {
                rx,
                cursor: None,
                discard: false,
            }),
            refresh_when_idle: true,
            ..Default::default()
        };
        state.close();
        tx.send(Ok(empty_page(None))).expect("send");
        state.drain_result();
        assert!(state.page.is_none());
        assert!(state.pending.is_none());
        assert!(!state.refresh_when_idle);
    }

    #[test]
    fn disconnected_worker_surfaces_error_without_erasing_last_page() {
        let (tx, rx) = mpsc::sync_channel(1);
        let mut state = RemoteEnvironments {
            open: true,
            page: Some(empty_page(None)),
            pending: Some(PendingLoad {
                rx,
                cursor: None,
                discard: false,
            }),
            ..Default::default()
        };
        drop(tx);
        state.drain_result();
        assert!(state.page.is_some());
        assert!(state.pending.is_none());
        assert_eq!(
            state.failure.as_ref().map(|failure| failure.error),
            Some(LoadError::WorkerUnavailable)
        );
    }

    #[test]
    fn modal_consumes_terminal_input_and_the_escape_dismissal_frame() {
        use crate::app::test_support::{raw_input, test_app};
        use crate::test_egui::DiscardTextures;
        let (_temp, mut app) = test_app();
        let ctx = Context::default();
        app.remote_environments.open = true;
        app.remote_environments.page = Some(empty_page(None));
        let _ = ctx
            .run_ui(raw_input([900.0, 680.0], None), |ui| {
                assert!(app.render_remote_environments(ui).is_some());
            })
            .discard_textures();
        let mut input = raw_input([900.0, 680.0], None);
        input.events.push(egui::Event::Text("must-not-reach-terminal".into()));
        input.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: Some(egui::Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        input
            .dropped_files
            .push(crate::app::test_support::dropped_file("/tmp/never-open-this-file"));
        let _ = ctx
            .run_ui(input, |ui| {
                assert!(app.render_remote_environments(ui).is_some());
                assert!(!app.remote_environments.open);
                ui.input(|input| {
                    assert!(input.events.is_empty());
                    assert!(input.raw.events.is_empty());
                    assert!(input.raw.dropped_files.is_empty());
                    assert!(input.keys_down.is_empty());
                });
                assert!(app.terminal_keyboard_events.is_empty());
                app.handle_shortcuts(ui);
                assert!(app.board.panels.is_empty());
            })
            .discard_textures();
    }

    #[test]
    fn loader_finds_owned_record_without_creating_a_local_session() {
        use crate::app::test_support::raw_input;
        use crate::test_egui::DiscardTextures;
        use horizon_core::remote_workspace::RemoteWorkspaceState;
        let temp = tempfile::tempdir().expect("tempdir");
        let home = HorizonHome::from_root(temp.path().join("home"));
        let store = CloudWorkflowStore::open(&home).expect("store");
        let owner = "00000000-0000-4000-8000-000000000001".to_string();
        let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version": 1,
            "spec": {
                "workspace_local_id": "detached-workspace",
                "target": { "provider": "local_docker", "profile": "development",
                    "image": format!("example/worker@sha256:{}", "a".repeat(64)),
                    "disk_gib": 20, "lifetime": "persistent" },
                "repository": { "repository": "example/project", "commit": "b".repeat(40) },
                "working_directory": ".", "generation": 0, "panels": []
            }
        }))
        .expect("state");
        let stored = store.create_remote_workspace(&owner, &state).expect("record");
        let page = load_page(&home, None).expect("page");
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].summary, stored.environment_summary());
        let ctx = Context::default();
        let mut view = RemoteEnvironments {
            open: true,
            page: Some(empty_page(None)),
            ..Default::default()
        };
        let render = |view: &RemoteEnvironments| {
            for _ in 0..2 {
                let _ = ctx
                    .run_ui(raw_input([900.0, 680.0], None), |ui| {
                        let _ = paint::show(ui, view);
                    })
                    .discard_textures();
            }
            ctx.data(|data| data.get_temp::<egui::Rect>(egui::Id::new("inventory-close-test")))
                .expect("close control")
                .top()
        };
        let empty_top = render(&view);
        view.page = Some(page);
        assert!(
            render(&view) < empty_top - 100.0,
            "populated dialog must grow beyond its cached empty height"
        );
        view.failure = Some(LoadFailure {
            error: LoadError::ReadPage,
            cursor: None,
        });
        assert!(render(&view) >= 32.0, "error controls must remain inside the viewport");
        assert!(!home.sessions_dir().exists());
        assert_eq!(
            store.load_remote_workspace(&owner, "detached-workspace").expect("load"),
            Some(stored)
        );
    }

    #[test]
    fn close_click_keeps_pointer_state_across_press_idle_and_release_frames() {
        use crate::app::test_support::{raw_input, test_app};
        use crate::test_egui::DiscardTextures;
        let (_temp, mut app) = test_app();
        let ctx = Context::default();
        app.remote_environments.open = true;
        app.remote_environments.page = Some(empty_page(None));
        let mut frame = |events| {
            let mut input = raw_input([900.0, 680.0], None);
            input.events = events;
            let _ = ctx
                .run_ui(input, |ui| {
                    let saved = app.render_remote_environments(ui).expect("modal frame");
                    assert!(!ui.input(|input| input.pointer.primary_down()));
                    HorizonApp::restore_remote_environment_input(ui, saved);
                })
                .discard_textures();
        };
        frame(Vec::new());
        frame(Vec::new());
        let position = ctx
            .data(|data| data.get_temp::<egui::Rect>(egui::Id::new("inventory-close-test")))
            .expect("close control")
            .center();
        let button = |pressed| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(position), button(true)]);
        frame(Vec::new());
        assert!(ctx.input(|input| input.pointer.primary_down()));
        frame(vec![egui::Event::PointerMoved(position)]);
        frame(vec![button(false)]);
        assert!(!app.remote_environments.open);
    }
}

//! Native file transfer routing and repaint state shared with the event loop.
use super::ObservedKeyboardInputs;
#[derive(Default)]
pub(super) struct NativeFileInputs {
    context: Option<egui::Context>,
    reset_worker: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    focused_surface: Option<u64>,
    wayland: bool,
    drops: std::collections::VecDeque<NativeDrop>,
    viewports: std::collections::HashMap<egui::ViewportId, u64>,
    windows: std::collections::HashMap<u64, (f64, Option<u64>)>,
    pastes: Vec<NativePaste>,
    requests: std::collections::HashMap<u64, PasteRequest>,
    next_request: u64,
    transfer_files: horizon_wayland::TransferFiles,
}

pub(crate) struct NativePaste {
    pub panel: horizon_core::PanelId,
    pub viewport: egui::ViewportId,
    pub paths: Vec<std::path::PathBuf>,
    pub text: Option<String>,
}

struct PasteRequest {
    panel: horizon_core::PanelId,
    viewport: egui::ViewportId,
    started: std::time::Instant,
}

struct NativeDrop {
    surface: u64,
    paths: Vec<std::path::PathBuf>,
    position: [f64; 2],
}

impl ObservedKeyboardInputs {
    #[cfg(target_os = "linux")]
    pub(crate) fn native_worker_resetter(&self, reset: impl Fn() + Send + Sync + 'static) {
        self.1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .reset_worker = Some(std::sync::Arc::new(reset));
    }

    pub(crate) fn reset_native_session(&self) {
        let (reset, context) = {
            let mut state = self.1.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            state.requests.clear();
            state.pastes.clear();
            state.drops.clear();
            for window in state.windows.values_mut() {
                window.1 = None;
            }
            (state.reset_worker.clone(), state.context.clone())
        };
        if let Some(reset) = reset {
            reset();
        }
        if let Some(ctx) = context {
            ctx.input_mut(|input| {
                input.raw.dropped_files.clear();
                input.raw.hovered_files.clear();
            });
        }
    }

    pub(crate) fn reset_native_transfers(&self) -> Vec<u64> {
        let mut state = self.1.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.requests.clear();
        state.pastes.clear();
        state.drops.clear();
        state.windows.keys().copied().collect()
    }

    pub(crate) fn decode_native_transfer(&self, mime: &str, bytes: &[u8]) -> std::io::Result<Vec<std::path::PathBuf>> {
        self.1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .transfer_files
            .decode_transfer_payload(mime, bytes)
    }

    pub(crate) fn clear_native_transfer_files(&self) {
        if let Err(error) = self
            .1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .transfer_files
            .clear()
        {
            tracing::warn!(%error, "failed to remove native image files during shutdown");
        }
    }

    pub(crate) fn wake_native_input(&self) {
        let context = self
            .1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .context
            .clone();
        if let Some(context) = context {
            context.request_repaint();
        }
    }
    pub(crate) fn native_context(&self, ctx: &egui::Context) -> bool {
        let mut state = self.1.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let first = state.context.is_none();
        state.context = Some(ctx.clone());
        first
    }
    pub(crate) fn set_wayland_backend(&self, wayland: bool) {
        self.1.lock().unwrap_or_else(std::sync::PoisonError::into_inner).wayland = wayland;
    }

    pub(crate) fn is_wayland_backend(&self) -> bool {
        self.1.lock().unwrap_or_else(std::sync::PoisonError::into_inner).wayland
    }

    pub(crate) fn native_window_seen(&self, surface: u64) {
        self.1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .windows
            .entry(surface)
            .or_insert((1.0, None));
    }

    pub(crate) fn native_window_scale(&self, surface: u64, scale: f64) {
        if let Some(window) = self
            .1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .windows
            .get_mut(&surface)
        {
            window.0 = scale;
        }
    }

    pub(crate) fn native_focus(&self, surface: u64, focused: bool) {
        let mut state = self.1.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if focused {
            state.focused_surface = Some(surface);
        } else {
            if state.focused_surface == Some(surface) {
                state.focused_surface = None;
            }
            if let Some(window) = state.windows.get_mut(&surface) {
                window.1 = None;
            }
        }
    }

    pub(crate) fn native_recipient_publisher(
        &self,
    ) -> impl Fn(egui::ViewportId, f64, Option<horizon_core::PanelId>) + Send + Sync + 'static {
        // egui owns this callback; a strong observer would make a cycle through
        // the repaint context stored in NativeFileInputs.
        let weak = std::sync::Arc::downgrade(&self.1);
        move |viewport, scale, target| {
            if let Some(state) = weak.upgrade() {
                let mut state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(surface) = state.focused_surface {
                    state.windows.insert(surface, (scale, target.map(|target| target.0)));
                    state.viewports.insert(viewport, surface);
                }
            }
        }
    }

    pub(crate) fn discard_native_drop_positions(&self, viewport: egui::ViewportId) {
        let mut state = self.1.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let surface = state.viewports.get(&viewport).copied();
        state
            .drops
            .retain(|drop| surface.is_some_and(|surface| drop.surface != surface));
    }

    pub(crate) fn native_drop_position(
        &self,
        surface: u64,
        position: [f64; 2],
        paths: Vec<std::path::PathBuf>,
    ) -> bool {
        let mut state = self.1.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.drops.len() >= 256 {
            return false;
        }
        state.drops.push_back(NativeDrop {
            surface,
            paths,
            position,
        });
        true
    }

    pub(crate) fn take_native_drop_batches(
        &self,
        viewport: egui::ViewportId,
        files: &[egui::DroppedFileHandle],
    ) -> Vec<(std::ops::Range<usize>, [f64; 2])> {
        let mut state = self.1.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let surface = state.viewports.get(&viewport).copied();
        let mut batches = Vec::new();
        let mut offset = 0;
        while offset < files.len() {
            let index = state.drops.iter().position(|drop| {
                surface.is_none_or(|surface| drop.surface == surface)
                    && !drop.paths.is_empty()
                    && files.get(offset..offset + drop.paths.len()).is_some_and(|files| {
                        drop.paths
                            .iter()
                            .map(std::path::PathBuf::as_path)
                            .eq(files.iter().map(|file| file.path()))
                    })
            });
            if let Some(drop) = index.and_then(|index| state.drops.remove(index)) {
                let end = offset + drop.paths.len();
                batches.push((offset..end, drop.position));
                offset = end;
            } else {
                // A raw file event with no current transfer record may belong
                // to an earlier session. Never route it using pointer state.
                offset += 1;
            }
        }
        batches
    }

    #[cfg(test)]
    pub(crate) fn take_native_drop_position(
        &self,
        viewport: egui::ViewportId,
        files: &[egui::DroppedFileHandle],
    ) -> Option<[f64; 2]> {
        self.take_native_drop_batches(viewport, files)
            .first()
            .map(|(_, position)| *position)
    }

    pub(crate) fn native_window(&self, surface: u64) -> Option<(f64, Option<u64>)> {
        self.1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .windows
            .get(&surface)
            .copied()
    }

    pub(crate) fn forget_native_window(&self, surface: u64) {
        let mut state = self.1.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.windows.remove(&surface);
        state.viewports.retain(|_, window| *window != surface);
        state.drops.retain(|drop| drop.surface != surface);
        if state.focused_surface == Some(surface) {
            state.focused_surface = None;
        }
    }

    pub(crate) fn native_paste_request(&self, surface: u64) -> Option<u64> {
        let mut state = self.1.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .requests
            .retain(|_, request| request.started.elapsed() < std::time::Duration::from_secs(15));
        if state.requests.len() >= 16 {
            return None;
        }
        let panel = horizon_core::PanelId(state.windows.get(&surface)?.1?);
        let viewport = *state.viewports.iter().find(|(_, window)| **window == surface)?.0;
        state.next_request = state.next_request.checked_add(1)?;
        let token = state.next_request;
        state.requests.insert(
            token,
            PasteRequest {
                panel,
                viewport,
                started: std::time::Instant::now(),
            },
        );
        Some(token)
    }

    pub(crate) fn cancel_native_paste_request(&self, token: u64) {
        self.1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .requests
            .remove(&token);
    }

    pub(crate) fn native_paste(&self, token: u64, paths: Vec<std::path::PathBuf>) {
        self.complete_native_paste(token, paths, None);
    }

    pub(crate) fn native_paste_text(&self, token: u64, text: String) {
        self.complete_native_paste(token, Vec::new(), Some(text));
    }

    fn complete_native_paste(&self, token: u64, paths: Vec<std::path::PathBuf>, text: Option<String>) {
        let context = {
            let mut state = self.1.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(request) = state.requests.remove(&token) else {
                return;
            };
            if paths.is_empty() && text.as_ref().is_none_or(String::is_empty) {
                return;
            }
            state.pastes.push(NativePaste {
                panel: request.panel,
                viewport: request.viewport,
                paths,
                text,
            });
            state.context.clone()
        };
        if let Some(context) = context {
            context.request_repaint();
        }
    }

    pub(crate) fn take_native_pastes(&self) -> Vec<NativePaste> {
        std::mem::take(&mut self.1.lock().unwrap_or_else(std::sync::PoisonError::into_inner).pastes)
    }
}

#[cfg(test)]
mod tests {
    #[derive(Debug)]
    struct TestFile(std::path::PathBuf);
    impl egui::DroppedFile for TestFile {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            Err("not read in routing test".into())
        }
    }

    #[test]
    fn native_transfers_keep_surface_and_recipient_until_consumed() {
        let observed = super::ObservedKeyboardInputs::default();
        observed.native_window_seen(10);
        observed.native_window_seen(20);
        observed.native_focus(10, true);
        observed.native_recipient_publisher()(egui::ViewportId::ROOT, 1.5, Some(horizon_core::PanelId(2)));
        let token = observed.native_paste_request(10).unwrap();
        observed.native_focus(10, false);
        observed.native_focus(20, true);
        observed.native_recipient_publisher()(egui::ViewportId::from_hash_of(20), 2.0, Some(horizon_core::PanelId(3)));
        assert_eq!(observed.native_window(10), Some((1.5, None)));
        assert_eq!(observed.native_window(20), Some((2.0, Some(3))));
        observed.native_paste(token, vec![std::path::PathBuf::from("/tmp/image.png")]);
        observed.native_focus(20, false);
        let pastes = observed.take_native_pastes();
        assert_eq!(pastes[0].panel, horizon_core::PanelId(2));
        assert_eq!(pastes[0].viewport, egui::ViewportId::ROOT);
        assert!(observed.take_native_pastes().is_empty());
        observed.forget_native_window(10);
        assert!(observed.native_window(10).is_none());
    }

    #[test]
    fn detached_paste_keeps_original_viewport_after_focus_changes() {
        let observed = super::ObservedKeyboardInputs::default();
        let child = egui::ViewportId::from_hash_of("detached");
        observed.native_window_seen(20);
        observed.native_focus(20, true);
        observed.native_recipient_publisher()(child, 1.0, Some(horizon_core::PanelId(3)));
        let token = observed.native_paste_request(20).unwrap();
        observed.forget_native_window(20);
        observed.native_window_seen(10);
        observed.native_focus(10, true);
        observed.native_recipient_publisher()(egui::ViewportId::ROOT, 1.0, Some(horizon_core::PanelId(4)));
        observed.native_paste(token, vec![std::path::PathBuf::from("/tmp/image.png")]);
        let paste = observed.take_native_pastes().pop().unwrap();
        assert_eq!(paste.panel, horizon_core::PanelId(3));
        assert_eq!(paste.viewport, child);
        assert!(observed.native_paste_request(20).is_none());
    }

    #[test]
    fn cancelled_and_reset_requests_cannot_deliver_later() {
        let observed = super::ObservedKeyboardInputs::default();
        observed.native_window_seen(10);
        observed.native_focus(10, true);
        observed.native_recipient_publisher()(egui::ViewportId::ROOT, 1.0, Some(horizon_core::PanelId(2)));
        let token = observed.native_paste_request(10).unwrap();
        observed.cancel_native_paste_request(token);
        observed.native_paste(token, vec![std::path::PathBuf::from("/tmp/image.png")]);
        assert!(observed.take_native_pastes().is_empty());
        let token = observed.native_paste_request(10).unwrap();
        assert_eq!(observed.reset_native_transfers(), vec![10]);
        observed.native_paste(token, vec![std::path::PathBuf::from("/tmp/image.png")]);
        assert!(observed.take_native_pastes().is_empty());
        assert!(observed.native_paste_request(10).is_some());
    }

    #[test]
    fn retained_context_callback_does_not_keep_native_input_alive() {
        let observed = super::ObservedKeyboardInputs::default();
        let context = egui::Context::default();
        observed.native_context(&context);
        let weak = std::sync::Arc::downgrade(&observed.1);
        let publish = observed.native_recipient_publisher();
        context.on_end_pass(
            "test native recipient",
            std::sync::Arc::new(move |_| {
                publish(egui::ViewportId::ROOT, 1.0, None);
            }),
        );
        drop(observed);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn completion_admission_never_evicts_an_accepted_drop() {
        let observed = super::ObservedKeyboardInputs::default();
        let path = std::path::PathBuf::from("/tmp/image.png");
        for _ in 0..256 {
            assert!(observed.native_drop_position(10, [1.0, 2.0], vec![path.clone()]));
        }
        assert!(!observed.native_drop_position(10, [3.0, 4.0], vec![path.clone()]));
        let files: Vec<egui::DroppedFileHandle> = (0..256)
            .map(|_| std::sync::Arc::new(TestFile(path.clone())) as egui::DroppedFileHandle)
            .collect();
        let batches = observed.take_native_drop_batches(egui::ViewportId::ROOT, &files);
        assert_eq!(batches.len(), 256);
        assert_eq!(
            batches.iter().map(|(_, position)| *position).collect::<Vec<_>>(),
            vec![[1.0, 2.0]; 256]
        );
    }

    #[test]
    fn text_fallback_keeps_the_original_recipient_and_cannot_cross_a_session_reset() {
        let observed = super::ObservedKeyboardInputs::default();
        observed.native_window_seen(10);
        observed.native_focus(10, true);
        observed.native_recipient_publisher()(egui::ViewportId::ROOT, 1.0, Some(horizon_core::PanelId(2)));
        let token = observed.native_paste_request(10).unwrap();
        observed.native_paste_text(token, "same-offer text".into());
        let paste = observed.take_native_pastes().pop().unwrap();
        assert_eq!(paste.panel, horizon_core::PanelId(2));
        assert_eq!(paste.text.as_deref(), Some("same-offer text"));
        let token = observed.native_paste_request(10).unwrap();
        observed.reset_native_session();
        observed.native_paste_text(token, "stale text".into());
        assert!(observed.take_native_pastes().is_empty());
    }

    #[test]
    fn concurrent_drop_batches_keep_each_coordinate_and_skip_stale_raw_events() {
        let observed = super::ObservedKeyboardInputs::default();
        let path = std::path::PathBuf::from("/tmp/same.png");
        let second = std::path::PathBuf::from("/tmp/second.png");
        observed.native_window_seen(10);
        observed.native_focus(10, true);
        observed.native_recipient_publisher()(egui::ViewportId::ROOT, 1.0, None);
        observed.native_drop_position(20, [999.0, 999.0], vec![path.clone()]);
        observed.native_drop_position(10, [1.0, 2.0], vec![path.clone()]);
        observed.native_drop_position(10, [3.0, 4.0], vec![path.clone(), second.clone()]);
        let files: Vec<egui::DroppedFileHandle> =
            [std::path::PathBuf::from("/tmp/stale.png"), path.clone(), path, second]
                .into_iter()
                .map(|path| std::sync::Arc::new(TestFile(path)) as egui::DroppedFileHandle)
                .collect();
        assert_eq!(
            observed.take_native_drop_batches(egui::ViewportId::ROOT, &files),
            vec![(1..2, [1.0, 2.0]), (2..4, [3.0, 4.0])]
        );
        assert!(
            observed
                .take_native_drop_batches(egui::ViewportId::ROOT, &files)
                .is_empty()
        );
    }

    #[test]
    fn ignored_drop_cannot_supply_coordinates_to_a_later_identical_path() {
        let observed = super::ObservedKeyboardInputs::default();
        let path = std::path::PathBuf::from("/tmp/a.png");
        observed.native_drop_position(10, [1.0, 2.0], vec![path.clone()]);
        observed.discard_native_drop_positions(egui::ViewportId::ROOT);
        let files: Vec<egui::DroppedFileHandle> = vec![std::sync::Arc::new(TestFile(path.clone()))];
        assert!(
            observed
                .take_native_drop_position(egui::ViewportId::ROOT, &files)
                .is_none()
        );
        observed.native_drop_position(10, [3.0, 4.0], vec![path]);
        assert_eq!(
            observed.take_native_drop_position(egui::ViewportId::ROOT, &files),
            Some([3.0, 4.0])
        );
    }

    #[test]
    fn asynchronous_drop_coordinates_follow_the_payload_into_its_viewport() {
        let observed = super::ObservedKeyboardInputs::default();
        let paths = vec![std::path::PathBuf::from("/tmp/a.png")];
        observed.native_drop_position(10, [120.0, 240.0], paths.clone());
        assert!(
            observed
                .take_native_drop_position(egui::ViewportId::ROOT, &[])
                .is_none()
        );
        let files: Vec<egui::DroppedFileHandle> = paths
            .into_iter()
            .map(|path| {
                let file: egui::DroppedFileHandle = std::sync::Arc::new(TestFile(path));
                file
            })
            .collect();
        assert_eq!(
            observed.take_native_drop_position(egui::ViewportId::ROOT, &files),
            Some([120.0, 240.0])
        );
        assert!(
            observed
                .take_native_drop_position(egui::ViewportId::ROOT, &files)
                .is_none()
        );
    }
}

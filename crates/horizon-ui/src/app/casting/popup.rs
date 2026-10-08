use super::{CastState, Picker, Session};
use egui::{Context, Pos2, Rect, Vec2, epaint::MarginF32};
use horizon_core::{
    WorkspaceId,
    browser::manifest::cast::{CastOperation, CastOrientation, CastResolution, CastSource},
};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SessionBinding {
    workspace: WorkspaceId,
    receiver_id: String,
    generation: Instant,
}
impl SessionBinding {
    pub(super) fn from_session(session: &Session) -> Self {
        Self {
            workspace: session.workspace,
            receiver_id: session.receiver_id.clone(),
            generation: session.generation,
        }
    }
    fn matches(&self, session: &Session) -> bool {
        self.workspace == session.workspace
            && self.receiver_id == session.receiver_id
            && self.generation == session.generation
    }
}
impl CastState {
    /// Opens the picker for `workspace`, or closes it when the same anchor opened it.
    /// A live session of the workspace keeps its source and settings; otherwise the
    /// picker starts on `source`.
    pub(super) fn toggle_picker(
        &mut self,
        anchor: Option<horizon_core::PanelId>,
        workspace: WorkspaceId,
        source: CastSource,
        ctx: &Context,
    ) {
        let reopened = self
            .picker
            .as_ref()
            .is_some_and(|picker| picker.anchor == anchor && picker.workspace == workspace && picker.anchor.is_some());
        self.close_picker(ctx);
        if reopened {
            return;
        }
        let session = self
            .sessions
            .iter()
            .rev()
            .find(|session| session.workspace == workspace && !session.worker.finished())
            .or_else(|| {
                self.sessions
                    .iter()
                    .rev()
                    .find(|session| session.workspace == workspace)
            });
        self.picker = Some(Picker {
            anchor,
            workspace,
            source: session.map_or(source, |session| session.source.clone()),
            receiver: session.map(|session| session.receiver_id.clone()),
            orientation: session.map_or(CastOrientation::Landscape, |session| session.orientation),
            resolution: session.map_or(CastResolution::default(), |session| session.resolution),
            pin: zeroize::Zeroizing::new(String::new()),
            position: None,
            binding: session.map(SessionBinding::from_session),
        });
        self.discover();
    }
    pub(super) fn bind_picker(&self, picker: &mut Picker) {
        picker.binding = self
            .sessions
            .iter()
            .find(|session| {
                session.workspace == picker.workspace && Some(&session.receiver_id) == picker.receiver.as_ref()
            })
            .map(SessionBinding::from_session);
    }
    pub(super) fn close_picker(&mut self, ctx: &Context) {
        if let Some(picker) = self.picker.take() {
            self.stop_picker_session(&picker, ctx);
        }
    }
    pub(super) fn stop_picker_session(&mut self, picker: &Picker, ctx: &Context) {
        if let Some(binding) = &picker.binding {
            for session in &mut self.sessions {
                if binding.matches(session) {
                    session.scaling = None;
                    session.worker.stop();
                }
            }
        }
        self.dismiss_picker(ctx);
    }
    pub(super) fn dismiss_picker(&mut self, ctx: &Context) {
        self.picker = None;
        // Keep known layers while egui retires their previous-frame areas.
        if let Some(menus) = self.control_menus {
            for menu in menus {
                egui::Popup::close_id(ctx, menu.id);
            }
        }
    }
}

pub(super) fn hide_after_action(source: &CastSource, action: &CastOperation, paired: bool) -> bool {
    matches!(source, CastSource::Application {})
        && (matches!(action, CastOperation::Pair { .. }) || (paired && matches!(action, CastOperation::Start { .. })))
}

pub(super) fn control_shadow_margin(ctx: &Context) -> MarginF32 {
    let style = ctx.global_style();
    let window = style.visuals.window_shadow.margin();
    let popup = style.visuals.popup_shadow.margin();
    // Include the raster edge and the caster even for negatively offset shadows.
    MarginF32 {
        left: window.left.max(popup.left).max(0.0) + 1.0,
        right: window.right.max(popup.right).max(0.0) + 1.0,
        top: window.top.max(popup.top).max(0.0) + 1.0,
        bottom: window.bottom.max(popup.bottom).max(0.0) + 1.0,
    }
}

pub(super) fn initial_position(canvas: Rect, source: Rect, size: Vec2, shadow: MarginF32) -> Pos2 {
    let gap = 4.0;
    let candidates = [
        egui::pos2(source.right() + shadow.left + gap, source.top()),
        egui::pos2(source.left() - size.x - shadow.right - gap, source.top()),
        egui::pos2(source.left(), source.bottom() + shadow.top + gap),
        egui::pos2(source.left(), source.top() - size.y - shadow.bottom - gap),
    ];
    candidates
        .into_iter()
        .find(|position| {
            let rect = Rect::from_min_size(*position, size);
            canvas.contains_rect(rect) && !source.intersects(rect + shadow)
        })
        .unwrap_or_else(|| {
            egui::pos2(
                source.right().min(canvas.right() - size.x).max(canvas.left()),
                (source.top() + 32.0).min(canvas.bottom() - size.y).max(canvas.top()),
            )
        })
}

#[cfg(test)]
mod tests;

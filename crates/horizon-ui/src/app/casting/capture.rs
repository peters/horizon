use super::super::HorizonApp;
#[cfg(test)]
use super::scaling::letterbox;
use egui::{Context, Id, LayerId, Order, Rect, ViewportId};
use horizon_cast::CastStatus;
use horizon_core::{WorkspaceId, browser::manifest::cast::CastSource};
use std::time::{Duration, Instant};

const CAPTURE_INTERVAL: Duration = Duration::from_millis(67);

#[derive(Clone, Copy)]
pub(super) struct RootGeometry {
    rect: Rect,
    pixels_per_point: u32,
    changed_at: Instant,
}
impl RootGeometry {
    fn observe(previous: Option<Self>, rect: Rect, pixels_per_point: f32, now: Instant) -> Self {
        let pixels_per_point = pixels_per_point.to_bits();
        if let Some(previous) = previous
            && previous.rect == rect
            && previous.pixels_per_point == pixels_per_point
        {
            previous
        } else {
            Self {
                rect,
                pixels_per_point,
                changed_at: now,
            }
        }
    }
    fn settled(self, rect: Rect, pixels_per_point: f32, now: Instant) -> bool {
        self.rect == rect
            && self.pixels_per_point == pixels_per_point.to_bits()
            && now.saturating_duration_since(self.changed_at) >= CAPTURE_INTERVAL
    }
}

#[derive(Clone, Debug)]
struct CaptureTicket {
    pixels_per_point: f32,
    regions: Vec<(String, Instant, Rect)>,
}

impl HorizonApp {
    pub(in crate::app) fn cast_frame(&mut self, ctx: &Context) {
        self.drain_cast_requests(ctx, None);
        if self.casting.poll() {
            ctx.request_repaint();
        }
        let mut source_lost = false;
        for session in &self.casting.sessions {
            if session.worker.finished() {
                continue;
            }
            if self.pending_session_switch.is_some()
                || self.cast_source_rect(session.workspace, &session.source, ctx).is_err()
            {
                session.worker.stop();
                source_lost = true;
            }
        }
        if source_lost {
            self.casting
                .notify("Casting stopped because its source is no longer visible".into());
        }
        self.casting.root_geometry = Some(RootGeometry::observe(
            self.casting.root_geometry,
            ctx.content_rect(),
            ctx.pixels_per_point(),
            Instant::now(),
        ));
        self.render_cast_controls(ctx);
        self.consume_cast_images(ctx);
        for session in &self.casting.sessions {
            session.worker.set_capture_paused(
                self.cast_source_rect(session.workspace, &session.source, ctx)
                    .is_ok_and(|rect| self.cast_controls_cover(rect, ctx))
                    || !self.cast_geometry_settled(session.workspace, &session.source, ctx),
            );
        }
        let active = !self.casting.finished();
        if active || self.casting.discovery.is_some() || self.casting.paired_refresh.is_some() {
            ctx.request_repaint_after(CAPTURE_INTERVAL);
        }
        if let Some(remaining) = self
            .casting
            .last_capture
            .and_then(|last| CAPTURE_INTERVAL.checked_sub(last.elapsed()))
        {
            ctx.request_repaint_after(remaining);
            return;
        }
        let mut regions = Vec::new();
        let mut source_obscured = false;
        for session in &self.casting.sessions {
            if !matches!(session.worker.status(), CastStatus::Streaming { .. }) {
                continue;
            }
            match self.cast_source_rect(session.workspace, &session.source, ctx) {
                Ok(rect) if !self.cast_obscured(&session.source, rect, ctx) => {
                    if !self.cast_geometry_settled(session.workspace, &session.source, ctx) {
                        continue;
                    }
                    if !self.cast_controls_cover(rect, ctx) {
                        regions.push((session.receiver_id.clone(), session.generation, rect));
                    }
                }
                _ => {
                    session.worker.stop();
                    source_obscured = true;
                }
            }
        }
        if source_obscured {
            self.casting
                .notify("Casting stopped because its source was hidden or covered".into());
        }
        if !regions.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(CaptureTicket {
                pixels_per_point: ctx.pixels_per_point(),
                regions,
            })));
            self.casting.last_capture = Some(capture_slot(self.casting.last_capture, Instant::now()));
        }
    }
    pub(super) fn cast_source_rect(
        &self,
        workspace: WorkspaceId,
        source: &CastSource,
        ctx: &Context,
    ) -> Result<Rect, String> {
        if matches!(source, CastSource::Application { .. }) {
            if self.board.workspace(workspace).is_none() {
                return Err("The casting workspace is no longer available".into());
            }
            if ctx.viewport_id() != ViewportId::ROOT
                || ctx.input(|input| input.viewport().minimized.unwrap_or(false))
                || self.startup_chooser.is_some()
                || self.shutdown_progress.is_some()
            {
                return Err("The main Horizon window is unavailable".into());
            }
            let rect = ctx.content_rect();
            return if rect.is_finite() && rect.width() >= 4.0 && rect.height() >= 4.0 {
                Ok(rect)
            } else {
                Err("The main Horizon window has no drawable area".into())
            };
        }
        #[cfg(feature = "cloud-workspaces")]
        if self.cloud_prototype.fullscreen.is_some() {
            return Err("Return to the main canvas before casting".into());
        }
        if self.workspace_is_detached(workspace)
            || self.startup_chooser.is_some()
            || self.shutdown_progress.is_some()
            || self.fullscreen_panel.is_some()
        {
            return Err("Source must be visible in the main Horizon window".into());
        }
        let selected: Vec<_> = match source {
            CastSource::Application {} => return Err("Invalid application capture context".into()),
            CastSource::Panel { id } => self
                .board
                .panels
                .iter()
                .filter(|panel| panel.workspace_id == workspace && panel.local_id == *id)
                .map(|panel| panel.id)
                .collect(),
            CastSource::Workspace { id } => {
                if self
                    .board
                    .workspace(workspace)
                    .is_none_or(|value| value.local_id != *id)
                {
                    return Err("Source is outside the current workspace".into());
                }
                self.board
                    .panels
                    .iter()
                    .filter(|panel| panel.workspace_id == workspace)
                    .map(|panel| panel.id)
                    .collect()
            }
        };
        // A workspace cast shows its clouds too, and a workspace may hold only clouds.
        let clouds = if matches!(source, CastSource::Workspace { .. }) {
            self.cast_cloud_rects(workspace, ctx)?
        } else {
            Vec::new()
        };
        if selected.is_empty() && clouds.is_empty() {
            return Err("Source has no visible content".into());
        }
        let mut bounds = clouds.into_iter().fold(Rect::NOTHING, Rect::union);
        for id in &selected {
            bounds = bounds.union(self.cast_visible_panel_rect(*id, ctx)?);
        }
        for panel in &self.board.panels {
            if !selected.contains(&panel.id)
                && self
                    .panel_screen_rects
                    .get(&panel.id)
                    .is_some_and(|rect| rect.intersects(bounds))
            {
                return Err("Another panel overlaps this source; move it before casting".into());
            }
        }
        if matches!(source, CastSource::Workspace { .. })
            && self
                .workspace_screen_rects
                .iter()
                .any(|(id, rect)| *id != workspace && rect.intersects(bounds))
        {
            return Err("Another workspace overlaps this source".into());
        }
        if !bounds.is_finite() || bounds.width() < 4.0 || bounds.height() < 4.0 {
            return Err("Source has no drawable area".into());
        }
        Ok(bounds.shrink(1.0))
    }
    fn cast_visible_panel_rect(&self, id: horizon_core::PanelId, ctx: &Context) -> Result<Rect, String> {
        if !self.panel_screen_rects.contains_key(&id) {
            return Err("Source is not visible".into());
        }
        let panel = self
            .board
            .panel(id)
            .filter(|panel| panel.visible)
            .ok_or("Source is not visible")?;
        let canvas = self.canvas_rect(ctx);
        let position = self.arranged_panel_position(
            id,
            panel.workspace_id,
            egui::pos2(panel.layout.position[0], panel.layout.position[1]),
        );
        // Hit-test rectangles are clipped. Validate complete render bounds before capture.
        let rect = Rect::from_min_size(
            self.canvas_to_screen(canvas, position),
            self.canvas_size_to_screen(egui::vec2(panel.layout.size[0], panel.layout.size[1])),
        );
        if !canvas.contains_rect(rect) {
            return Err("Fit the entire source into view before casting".into());
        }
        Ok(rect)
    }
    fn cast_geometry_settled(&self, workspace: WorkspaceId, source: &CastSource, ctx: &Context) -> bool {
        if matches!(source, CastSource::Application { .. }) {
            return self.cast_source_rect(workspace, source, ctx).is_ok()
                && self.casting.root_geometry.is_some_and(|geometry| {
                    geometry.settled(ctx.content_rect(), ctx.pixels_per_point(), Instant::now())
                });
        }
        if matches!(source, CastSource::Workspace { .. }) && !self.cast_clouds_settled(workspace, ctx) {
            return false;
        }
        self.board
            .panels
            .iter()
            .filter(|panel| {
                panel.workspace_id == workspace
                    && match source {
                        CastSource::Panel { id } => panel.local_id == *id,
                        CastSource::Workspace { .. } => true,
                        CastSource::Application {} => false,
                    }
            })
            .all(|panel| {
                self.panel_screen_rects.get(&panel.id).is_some_and(|rendered| {
                    self.cast_visible_panel_rect(panel.id, ctx)
                        .is_ok_and(|current| current == *rendered)
                })
            })
    }
    pub(super) fn cast_controls_cover(&self, rect: Rect, ctx: &Context) -> bool {
        let shadow = super::popup::control_shadow_margin(ctx);
        ctx.memory(|memory| {
            memory
                .areas()
                .visible_layer_ids()
                .into_iter()
                .filter(|layer| {
                    self.is_cast_control_layer(*layer)
                        || memory.areas().parent_layer(*layer) == Some(cast_picker_layer())
                })
                .any(|layer| {
                    memory.area_rect(layer.id).is_some_and(|area| {
                        let transform = memory.to_global.get(&layer).copied().unwrap_or_default();
                        (transform * (area + shadow)).intersects(rect)
                    })
                })
        })
    }
    pub(super) fn is_cast_control_layer(&self, layer: LayerId) -> bool {
        layer == cast_picker_layer() || self.casting.control_menus.is_some_and(|menus| menus.contains(&layer))
    }
    fn cast_obscured(&self, source: &CastSource, rect: Rect, ctx: &Context) -> bool {
        if self.pending_session_switch.is_some() {
            return true;
        }
        if matches!(source, CastSource::Application { .. }) {
            return false;
        }
        if self.host_content_dialog_open() {
            return true;
        }
        let cast_workspace = match source {
            CastSource::Workspace { id } => self.board.workspace_id_by_local_id(id),
            _ => None,
        };
        ctx.memory(|memory| {
            memory.areas().visible_layer_ids().into_iter().any(|layer| {
                if layer.order == Order::Background
                    || self.is_cast_control_layer(layer)
                    || memory.areas().parent_layer(layer) == Some(cast_picker_layer())
                    || cast_workspace.is_some_and(|workspace| self.cast_cloud_layer(workspace, layer))
                {
                    return false;
                }
                let selected = self.board.panels.iter().filter(|panel| match source {
                    CastSource::Application {} => false,
                    CastSource::Panel { id } => panel.local_id == *id,
                    CastSource::Workspace { id } => self
                        .board
                        .workspace(panel.workspace_id)
                        .is_some_and(|value| value.local_id == *id),
                });
                // A selected panel's own layers, including sublayers such as its resize grip,
                // are part of the picture.
                let parent = memory.areas().parent_layer(layer);
                if selected.clone().any(|panel| {
                    let own = Id::new(("panel", panel.id.0));
                    layer.id == own
                        || layer.id == Id::new(("cast_icon", panel.id.0))
                        || layer.id == Id::new(("panel_resize_layer", panel.id.0))
                        || parent.is_some_and(|parent| parent.id == own)
                }) {
                    return false;
                }
                let Some(area) = memory.area_rect(layer.id) else {
                    return false;
                };
                let transform = memory.to_global.get(&layer).copied().unwrap_or_default();
                (transform * area).intersects(rect)
            })
        })
    }
    fn consume_cast_images(&mut self, ctx: &Context) {
        if !self
            .casting
            .sessions
            .iter()
            .any(|session| matches!(session.worker.status(), CastStatus::Streaming { .. }))
        {
            return;
        }
        for session in &self.casting.sessions {
            if let Some(frame) = session.scaling.as_ref().and_then(super::scaling::Scaler::take)
                && matches!(session.worker.status(), CastStatus::Streaming { .. })
                && frame.pixels_per_point.to_bits() == ctx.pixels_per_point().to_bits()
                && self
                    .cast_source_rect(session.workspace, &session.source, ctx)
                    .is_ok_and(|rect| rect == frame.rect)
                && self.cast_geometry_settled(session.workspace, &session.source, ctx)
                && !self.cast_controls_cover(frame.rect, ctx)
                && !self.cast_obscured(&session.source, frame.rect, ctx)
            {
                let _ = session.worker.submit_source(frame.width, frame.height, frame.rgba);
            }
        }
        let events = ctx.input(|input| input.events.clone());
        for event in events {
            let egui::Event::Screenshot {
                viewport_id,
                user_data,
                image,
            } = event
            else {
                continue;
            };
            if viewport_id != ViewportId::ROOT {
                continue;
            }
            let Some(ticket) = user_data
                .data
                .as_ref()
                .and_then(|value| value.downcast_ref::<CaptureTicket>())
            else {
                continue;
            };
            if ticket.pixels_per_point.to_bits() != ctx.pixels_per_point().to_bits() {
                continue;
            }
            let (Ok(width), Ok(height)) = (u16::try_from(image.width()), u16::try_from(image.height())) else {
                continue;
            };
            let image_bounds = Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(f32::from(width), f32::from(height)) / ticket.pixels_per_point,
            );
            for (receiver_id, generation, rect) in &ticket.regions {
                let Some(session) = self
                    .casting
                    .sessions
                    .iter()
                    .find(|session| session.receiver_id == *receiver_id && session.generation == *generation)
                else {
                    continue;
                };
                if !matches!(session.worker.status(), CastStatus::Streaming { .. }) {
                    continue;
                }
                let Ok(current) = self.cast_source_rect(session.workspace, &session.source, ctx) else {
                    continue;
                };
                if current != *rect
                    || !self.cast_geometry_settled(session.workspace, &session.source, ctx)
                    || self.cast_controls_cover(current, ctx)
                    || self.cast_obscured(&session.source, current, ctx)
                {
                    continue;
                }
                if !image_bounds.contains_rect(*rect) {
                    session.worker.stop();
                    continue;
                }
                if let Some(scaling) = &session.scaling {
                    scaling.submit(
                        image.clone(),
                        *rect,
                        ticket.pixels_per_point,
                        session.worker.uses_source_frames(),
                    );
                }
            }
        }
    }
}
fn capture_slot(previous: Option<Instant>, now: Instant) -> Instant {
    // Preserve phase after a slightly late frame; longer stalls must not accumulate catch-up captures.
    if let Some(next) = previous.and_then(|previous| previous.checked_add(CAPTURE_INTERVAL))
        && next <= now
        && now.duration_since(next) < CAPTURE_INTERVAL
    {
        next
    } else {
        now
    }
}
fn cast_picker_layer() -> LayerId {
    LayerId::new(Order::Foreground, Id::new("cast_picker"))
}

#[cfg(test)]
mod tests;

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
        self.consume_cast_images(ctx);
        self.render_cast_controls(ctx);
        let controls_visible = self.cast_controls_visible(ctx);
        for session in &self.casting.sessions {
            session.worker.set_capture_paused(
                controls_visible || !self.cast_geometry_settled(session.workspace, &session.source, ctx),
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
                    if !controls_visible {
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
        if selected.is_empty() {
            return Err("Source has no visible panels".into());
        }
        let mut bounds = Rect::NOTHING;
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
    fn cast_controls_visible(&self, ctx: &Context) -> bool {
        self.casting.picker.is_some()
            || ctx.memory(|memory| {
                memory
                    .areas()
                    .visible_layer_ids()
                    .into_iter()
                    .any(|layer| self.is_cast_control_layer(layer))
            })
    }
    fn is_cast_control_layer(&self, layer: LayerId) -> bool {
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
        ctx.memory(|memory| {
            memory.areas().visible_layer_ids().into_iter().any(|layer| {
                if layer.order == Order::Background
                    || self.is_cast_control_layer(layer)
                    || memory.areas().parent_layer(layer) == Some(cast_picker_layer())
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
                if selected.clone().any(|panel| {
                    layer.id == Id::new(("panel", panel.id.0)) || layer.id == Id::new(("cast_icon", panel.id.0))
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
        if self.cast_controls_visible(ctx)
            || !self
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
                && !self.cast_obscured(&session.source, frame.rect, ctx)
            {
                let _ = session.worker.submit(frame.rgba);
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
                    || self.cast_obscured(&session.source, current, ctx)
                {
                    continue;
                }
                if !image_bounds.contains_rect(*rect) {
                    session.worker.stop();
                    continue;
                }
                if let Some(scaling) = &session.scaling {
                    scaling.submit(image.clone(), *rect, ticket.pixels_per_point);
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
mod tests {
    use super::*;
    use crate::app::test_support::{
        editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup,
    };
    use crate::test_egui::DiscardTextures;
    use horizon_core::{RuntimeState, StartupDecision};

    #[test]
    fn application_capture_uses_the_root_window_and_includes_its_dialogs() {
        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        let workspace = WorkspaceId(1);
        for size in [[1600.0, 1000.0], [800.0, 1200.0]] {
            let _ = ctx
                .run_ui(raw_input(size, None), |ui| {
                    let rect = app
                        .cast_source_rect(workspace, &CastSource::Application {}, ui.ctx())
                        .expect("root source without panels");
                    assert_eq!(
                        rect,
                        Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]))
                    );
                    egui::Window::new("Synthetic dialog").show(ui.ctx(), |ui| {
                        ui.label("Part of this window");
                    });
                    assert!(!app.cast_obscured(&CastSource::Application {}, rect, ui.ctx()));
                    app.casting.root_geometry = Some(RootGeometry::observe(
                        None,
                        rect,
                        ui.ctx().pixels_per_point(),
                        Instant::now()
                            .checked_sub(CAPTURE_INTERVAL)
                            .expect("capture interval fits"),
                    ));
                    assert!(app.cast_geometry_settled(workspace, &CastSource::Application {}, ui.ctx()));
                    assert!(app.cast_obscured(&CastSource::Panel { id: "synthetic".into() }, rect, ui.ctx()));
                })
                .discard_textures();
        }
        let mut minimized = raw_input([800.0, 1200.0], None);
        minimized.viewports.entry(ViewportId::ROOT).or_default().minimized = Some(true);
        let _ = ctx
            .run_ui(minimized, |ui| {
                assert!(
                    app.cast_source_rect(workspace, &CastSource::Application {}, ui.ctx())
                        .is_err()
                );
            })
            .discard_textures();
    }

    #[test]
    fn sustained_root_resize_and_density_changes_keep_capture_frozen_until_settled() {
        let start = Instant::now();
        let mut geometry = None;
        // Five seconds of changing bounds must not expose the encoder's three-second idle timeout.
        for step in 0_u16..=100 {
            let now = start + Duration::from_millis(u64::from(step) * 50);
            let rect = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0 + f32::from(step), 1000.0));
            let current = RootGeometry::observe(geometry, rect, 1.0, now);
            assert!(!current.settled(rect, 1.0, now));
            geometry = Some(current);
        }
        let current = geometry.expect("observed root");
        assert!(current.settled(current.rect, 1.0, start + Duration::from_secs(6)));
        let density_changed = RootGeometry::observe(Some(current), current.rect, 2.0, start + Duration::from_secs(6));
        assert!(!density_changed.settled(current.rect, 2.0, start + Duration::from_secs(6)));
        assert!(!current.settled(current.rect, 2.0, start + Duration::from_secs(7)));
        assert!(density_changed.settled(current.rect, 2.0, start + Duration::from_secs(7)));
    }

    #[test]
    fn slightly_late_ui_frames_preserve_the_capture_cadence() {
        let start = Instant::now();
        let mut previous = None;
        let mut captures = 0;
        for frame in 0..=100 {
            let now = start + Duration::from_millis(frame * 60);
            if previous.is_none_or(|previous| now.duration_since(previous) >= CAPTURE_INTERVAL) {
                previous = Some(capture_slot(previous, now));
                captures += 1;
            }
        }
        assert_eq!(captures, 90);
    }

    #[test]
    fn a_long_capture_stall_reanchors_without_a_catch_up_burst() {
        let previous = Instant::now();
        let now = previous + Duration::from_millis(300);
        assert_eq!(capture_slot(Some(previous), now), now);
        assert_eq!(capture_slot(None, now), now);
    }

    #[test]
    fn letterbox_preserves_aspect_ratio_and_opaque_borders() {
        let image = egui::ColorImage::filled([4, 2], egui::Color32::RED);
        let out = letterbox(&image, (4, 4));
        assert_eq!(&out[..16], &[0, 0, 0, 255].repeat(4));
        assert_eq!(&out[16..48], &[255, 0, 0, 255].repeat(8));
        assert_eq!(&out[48..], &[0, 0, 0, 255].repeat(4));
    }
    #[test]
    fn letterbox_matches_nearest_pixels_for_upscaling_downscaling_and_portrait() {
        let image = egui::ColorImage::new(
            [3, 2],
            vec![
                egui::Color32::RED,
                egui::Color32::GREEN,
                egui::Color32::BLUE,
                egui::Color32::WHITE,
                egui::Color32::BLACK,
                egui::Color32::YELLOW,
            ],
        );
        for (width, height) in [(12, 8), (8, 12), (2, 2), (3, 2)] {
            let actual = letterbox(&image, (width, height));
            let (fit_width, fit_height) = if image.width() * height > image.height() * width {
                (width, image.height() * width / image.width())
            } else {
                (image.width() * height / image.height(), height)
            };
            let left = (width - fit_width) / 2;
            let top = (height - fit_height) / 2;
            for y in 0..height {
                for x in 0..width {
                    let expected = if x >= left && x < left + fit_width && y >= top && y < top + fit_height {
                        image[(
                            (x - left) * image.width() / fit_width,
                            (y - top) * image.height() / fit_height,
                        )]
                            .to_array()
                    } else {
                        [0, 0, 0, 255]
                    };
                    let at = (y * width + x) * 4;
                    assert_eq!(&actual[at..at + 4], &expected);
                }
            }
        }
    }

    #[test]
    fn layout_changes_wait_for_matching_rendered_geometry() {
        let state = RuntimeState {
            workspaces: vec![editor_workspace_state("source", [0.0, 0.0])],
            ..RuntimeState::default()
        };
        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(state),
        });
        run_app_frame_with_input(&ctx, &mut app, raw_input([1600.0, 1000.0], None));
        let workspace = app.board.workspaces[0].id;
        let id = app.board.panels[0].id;
        let source = CastSource::Panel {
            id: "source-panel".into(),
        };
        assert!(app.cast_geometry_settled(workspace, &source, &ctx));
        for resize in [false, true] {
            if resize {
                app.board.panels[0].layout.size[0] += 20.0;
            } else {
                app.board.panels[0].layout.position[0] += 20.0;
            }
            assert!(app.cast_source_rect(workspace, &source, &ctx).is_ok());
            assert!(!app.cast_geometry_settled(workspace, &source, &ctx));
            let rendered = app.cast_visible_panel_rect(id, &ctx).expect("visible source");
            app.panel_screen_rects.insert(id, rendered);
            assert!(app.cast_geometry_settled(workspace, &source, &ctx));
        }
    }

    #[test]
    fn cast_controls_freeze_capture_but_other_overlays_remain_private() {
        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        let source = CastSource::Panel { id: "synthetic".into() };
        let region = Rect::from_min_size(egui::pos2(400.0, 300.0), egui::vec2(200.0, 150.0));
        for other_overlay in [false, true] {
            let _ = ctx
                .run_ui(raw_input([1600.0, 1000.0], None), |ui| {
                    egui::Area::new(cast_picker_layer().id)
                        .order(Order::Foreground)
                        .fixed_pos(region.min)
                        .show(ui.ctx(), |ui| {
                            ui.allocate_space(region.size());
                        });
                    let child = egui::Area::new(Id::new("cast_menu"))
                        .order(Order::Foreground)
                        .fixed_pos(region.min)
                        .show(ui.ctx(), |ui| {
                            ui.allocate_space(region.size());
                        });
                    ui.ctx().set_sublayer(cast_picker_layer(), child.response.layer_id);
                    app.casting.control_menus = Some([
                        child.response.layer_id,
                        LayerId::new(Order::Foreground, Id::new("second_cast_menu")),
                    ]);
                    if other_overlay {
                        egui::Area::new(Id::new("unrelated_overlay"))
                            .order(Order::Foreground)
                            .fixed_pos(region.min)
                            .show(ui.ctx(), |ui| {
                                ui.allocate_space(region.size());
                            });
                    }
                    assert!(app.cast_controls_visible(ui.ctx()));
                    assert_eq!(app.cast_obscured(&source, region, ui.ctx()), other_overlay);
                })
                .discard_textures();
            // egui has discarded sublayers but retained prior-frame areas.
            assert!(app.cast_controls_visible(&ctx));
            assert_eq!(app.cast_obscured(&source, region, &ctx), other_overlay);
        }
    }

    #[test]
    fn source_authority_and_overlay_privacy_are_checked() {
        let state = RuntimeState {
            workspaces: vec![
                editor_workspace_state("first", [0.0, 0.0]),
                editor_workspace_state("second", [600.0, 0.0]),
            ],
            ..RuntimeState::default()
        };
        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(state),
        });
        run_app_frame_with_input(&ctx, &mut app, raw_input([1600.0, 1000.0], None));
        let first = app.board.workspaces[0].id;
        let source = CastSource::Panel {
            id: "first-panel".into(),
        };
        assert!(
            app.cast_source_rect(
                first,
                &CastSource::Panel {
                    id: "second-panel".into()
                },
                &ctx
            )
            .is_err()
        );
        let region = Rect::from_min_size(egui::pos2(400.0, 300.0), egui::vec2(200.0, 150.0));
        let _ = ctx.run_ui(raw_input([1600.0, 1000.0], None), |ui| {
            egui::Area::new(Id::new("synthetic_overlay"))
                .order(Order::Foreground)
                .fixed_pos(region.min)
                .show(ui.ctx(), |ui| {
                    ui.allocate_space(region.size());
                });
        });
        assert!(app.cast_obscured(&source, region, &ctx));
        app.board.panels[0].layout.size[0] = 10_000.0;
        let id = app.board.panels[0].id;
        app.panel_screen_rects.insert(id, region);
        assert!(
            app.cast_source_rect(first, &source, &ctx)
                .expect_err("clipped source")
                .contains("Fit the entire source")
        );
        assert!(!app.cast_obscured(
            &source,
            Rect::from_min_size(egui::pos2(1200.0, 700.0), egui::vec2(20.0, 20.0)),
            &ctx
        ));
    }
}

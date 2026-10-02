use super::{super::HorizonApp, Picker};
use egui::{Context, Id, Order, Pos2, Rect, Sense, Stroke, Vec2};
use horizon_core::browser::manifest::cast::{CastOperation, CastOrientation, CastResolution, CastSource};

impl HorizonApp {
    pub(super) fn render_cast_controls(&mut self, ctx: &Context) {
        if self.startup_chooser.is_some() || self.shutdown_progress.is_some() {
            return;
        }
        self.render_cast_icons(ctx);
        let Some(mut picker) = self.casting.picker.take() else {
            return;
        };
        let snapshot = self.cast_snapshot(picker.workspace, ctx);
        let mut open = true;
        let mut action = None;
        let mut close_after = false;
        let window = egui::Window::new("Cast")
            .id(Id::new("cast_picker"))
            .order(Order::Foreground)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(320.0)
            .default_pos(egui::pos2(
                (self.canvas_rect(ctx).right() - 340.0).max(self.canvas_rect(ctx).left() + 12.0),
                self.canvas_rect(ctx).top() + 40.0,
            ))
            .show(ctx, |ui| {
                render_selection(ui, &mut picker, &snapshot, &mut action);
                let current = snapshot
                    .sessions
                    .iter()
                    .find(|session| Some(&session.receiver_id) == picker.receiver.as_ref());
                if let Some(session) = current {
                    ui.label(match session.state.as_str() {
                        "pin_required" => "Waiting for TV code",
                        "connecting" => "Connecting…",
                        "starting" => "Starting…",
                        "streaming" => "Casting",
                        "stopping" => "Stopping…",
                        "stopped" => "Stopped",
                        _ => "Could not cast",
                    });
                    render_encoder_status(ui, session);
                    if let Some(error) = &session.error {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                    if session.state == "pin_required" {
                        ui.label("Enter the code shown on your TV");
                        ui.add(
                            egui::TextEdit::singleline(&mut *picker.pin)
                                .password(true)
                                .char_limit(4),
                        );
                        if ui.button("Pair").clicked() {
                            action = Some(CastOperation::Pair {
                                receiver_id: session.receiver_id.clone(),
                                pin: picker.pin.to_string(),
                            });
                            close_after = true;
                        }
                    }
                    if !matches!(session.state.as_str(), "stopped" | "failed") && ui.button("Stop casting").clicked() {
                        action = Some(CastOperation::Stop {
                            receiver_id: session.receiver_id.clone(),
                        });
                    }
                }
                let busy = current.is_some_and(|session| !matches!(session.state.as_str(), "stopped" | "failed"));
                if ui
                    .add_enabled(picker.receiver.is_some() && !busy, egui::Button::new("Start casting"))
                    .clicked()
                    && let Some(receiver) = &picker.receiver
                {
                    picker.pin = zeroize::Zeroizing::new(String::new());
                    close_after = snapshot.paired_receivers.iter().any(|paired| &paired.id == receiver);
                    action = Some(CastOperation::Start {
                        receiver_id: receiver.clone(),
                        source: picker.source.clone(),
                        orientation: picker.orientation,
                        resolution: picker.resolution,
                    });
                }
                if let Some(notice) = &self.casting.notice {
                    ui.colored_label(ui.visuals().error_fg_color, notice);
                }
            });
        if let Some(window) = window
            && !egui::Popup::is_any_open(ctx)
        {
            ctx.move_to_top(window.response.layer_id);
        }
        if let Some(action) = action {
            let outcome = self.cast_operation(picker.workspace, &action, ctx);
            self.casting.notice = outcome.error;
            if self.casting.notice.is_some() {
                close_after = false;
            }
        }
        if open && !close_after {
            self.casting.picker = Some(picker);
        }
    }
    fn render_cast_icons(&mut self, ctx: &Context) {
        if !self.host_dialog_open() && self.casting.picker.is_none() {
            let panels: Vec<_> = self
                .board
                .panels
                .iter()
                .filter_map(|panel| {
                    self.panel_screen_rects
                        .get(&panel.id)
                        .map(|rect| (panel.id, panel.local_id.clone(), panel.workspace_id, *rect))
                })
                .collect();
            for (id, local_id, workspace, rect) in panels {
                if self.workspace_is_detached(workspace) || !self.canvas_rect(ctx).intersects(rect) {
                    continue;
                }
                let scale = self.canvas_view.zoom;
                let position = Pos2::new(rect.right() - 80.0 * scale, rect.top() + 7.0 * scale);
                let order = if self.board.focused == Some(id) {
                    Order::Foreground
                } else {
                    Order::Middle
                };
                let response = egui::Area::new(Id::new(("cast_icon", id.0)))
                    .order(order)
                    .fixed_pos(position)
                    .show(ctx, |ui| {
                        let (button, response) = ui.allocate_exact_size(Vec2::splat(20.0 * scale), Sense::click());
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Cast panel or workspace")
                        });
                        let active = self
                            .casting
                            .sessions
                            .iter()
                            .any(|session| session.workspace == workspace && !session.worker.finished());
                        let color = if active {
                            ui.visuals().selection.stroke.color
                        } else if response.hovered() {
                            ui.visuals().strong_text_color()
                        } else {
                            ui.visuals().weak_text_color()
                        };
                        if response.hovered() || active {
                            ui.painter().rect_filled(
                                button.expand(3.0 * scale),
                                5.0 * scale,
                                color.gamma_multiply(if active { 0.14 } else { 0.08 }),
                            );
                        }
                        paint_cast_icon(ui.painter(), button, color);
                        response.on_hover_text("Cast panel or workspace")
                    });
                ctx.set_sublayer(
                    egui::LayerId::new(order, Id::new(("panel", id.0))),
                    response.response.layer_id,
                );
                if response.inner.clicked() {
                    let session = self
                        .casting
                        .sessions
                        .iter()
                        .rev()
                        .find(|session| session.workspace == workspace && !session.worker.finished())
                        .or_else(|| {
                            self.casting
                                .sessions
                                .iter()
                                .rev()
                                .find(|session| session.workspace == workspace)
                        });
                    self.casting.picker = Some(Picker {
                        workspace,
                        source: CastSource::Panel { id: local_id },
                        receiver: session.map(|session| session.receiver_id.clone()),
                        orientation: session.map_or(CastOrientation::Landscape, |session| session.orientation),
                        resolution: session.map_or(CastResolution::default(), |session| session.resolution),
                        pin: zeroize::Zeroizing::new(String::new()),
                    });
                    self.casting.discover();
                }
            }
        }
    }
}

fn paint_cast_icon(painter: &egui::Painter, rect: Rect, color: egui::Color32) {
    let scale = rect.width() / 20.0;
    let point = |x, y| rect.min + egui::vec2(x, y) * scale;
    let stroke = Stroke::new(1.5 * scale, color);
    // Keep the display open around the signal so the glyph remains readable at header size.
    painter.add(egui::Shape::line(
        vec![
            point(3.0, 8.0),
            point(3.0, 4.5),
            point(3.4, 3.4),
            point(4.5, 3.0),
            point(16.5, 3.0),
            point(17.6, 3.4),
            point(18.0, 4.5),
            point(18.0, 14.5),
            point(17.6, 15.6),
            point(16.5, 16.0),
            point(12.0, 16.0),
        ],
        stroke,
    ));
    for radius in [4.0, 7.0] {
        let arc = (0_u8..=12)
            .map(|step| {
                let angle = std::f32::consts::FRAC_PI_2 * f32::from(step) / 12.0;
                point(3.0 + radius * angle.sin(), 17.0 - radius * angle.cos())
            })
            .collect();
        painter.add(egui::Shape::line(arc, stroke));
    }
    painter.circle_filled(point(3.0, 17.0), 1.1 * scale, color);
}

fn render_encoder_status(ui: &mut egui::Ui, session: &horizon_core::browser::manifest::cast::CastSessionInfo) {
    if let Some(encoder) = &session.encoder {
        ui.label(if encoder == "h264_nvenc" {
            "Encoder: NVIDIA NVENC"
        } else {
            "Encoder: CPU"
        });
    }
    if let Some(reason) = &session.encoder_fallback {
        ui.label(reason);
    }
}

fn render_selection(
    ui: &mut egui::Ui,
    picker: &mut Picker,
    snapshot: &horizon_core::browser::manifest::cast::CastOutcome,
    action: &mut Option<CastOperation>,
) -> Id {
    ui.label("Resolution");
    ui.horizontal(|ui| {
        for (resolution, label) in [
            (CastResolution::Hd720, "720p"),
            (CastResolution::FullHd1080, "1080p"),
            (CastResolution::Uhd4k, "4K"),
        ] {
            ui.selectable_value(&mut picker.resolution, resolution, label);
        }
    });
    ui.add_space(8.0);
    let parent_layer = ui.layer_id();
    ui.label("Source");
    egui::ComboBox::from_id_salt("cast_source")
        .selected_text(
            snapshot
                .sources
                .iter()
                .find(|source| source.source == picker.source)
                .map_or("Select source", |source| source.name.as_str()),
        )
        .show_ui(ui, |ui| {
            ui.ctx().set_sublayer(parent_layer, ui.layer_id());
            for source in &snapshot.sources {
                ui.selectable_value(&mut picker.source, source.source.clone(), &source.name);
            }
        });
    ui.add_space(8.0);
    ui.label("Apple TV");
    let receiver = egui::ComboBox::from_id_salt("cast_receiver")
        .selected_text(
            snapshot
                .receivers
                .iter()
                .find(|receiver| Some(&receiver.id) == picker.receiver.as_ref())
                .map_or("Select Apple TV", |receiver| receiver.name.as_str()),
        )
        .show_ui(ui, |ui| {
            ui.ctx().set_sublayer(parent_layer, ui.layer_id());
            for receiver in &snapshot.receivers {
                ui.selectable_value(&mut picker.receiver, Some(receiver.id.clone()), &receiver.name);
            }
        });
    if snapshot.discovering {
        ui.spinner();
    } else if ui.small_button("Refresh TVs").clicked() {
        *action = Some(CastOperation::Discover);
    }
    ui.horizontal(|ui| {
        ui.selectable_value(&mut picker.orientation, CastOrientation::Landscape, "Landscape");
        ui.selectable_value(&mut picker.orientation, CastOrientation::Portrait, "Portrait");
    });
    receiver.response.id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;
    use horizon_core::{
        WorkspaceId,
        browser::manifest::cast::{CastOutcome, CastReceiver},
    };

    #[test]
    fn cast_icon_stays_above_panel_after_drag_raises_it() {
        let ctx = Context::default();
        for order in [Order::Middle, Order::Foreground] {
            let parent = egui::LayerId::new(order, Id::new(("panel", 1)));
            let icon = egui::LayerId::new(order, Id::new(("cast_icon", 1)));
            let mut button = Rect::NOTHING;
            for _ in 0..4 {
                let input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                    ..egui::RawInput::default()
                };
                let _ = ctx
                    .run_ui(input, |ui| {
                        egui::Area::new(parent.id)
                            .order(order)
                            .fixed_pos(Pos2::new(50.0, 50.0))
                            .show(ui.ctx(), |ui| {
                                ui.allocate_exact_size(Vec2::new(300.0, 200.0), Sense::drag());
                            });
                        let response = egui::Area::new(icon.id)
                            .order(order)
                            .fixed_pos(Pos2::new(250.0, 60.0))
                            .show(ui.ctx(), |ui| {
                                ui.allocate_exact_size(Vec2::splat(20.0), Sense::click()).0
                            });
                        button = response.inner;
                        ui.ctx().set_sublayer(parent, response.response.layer_id);
                        ui.ctx().move_to_top(parent);
                    })
                    .discard_textures();
            }
            assert_eq!(ctx.layer_id_at(button.center()), Some(icon));
        }
    }

    #[test]
    fn receiver_menu_stays_above_picker_after_window_is_raised() {
        let ctx = Context::default();
        let mut picker = Picker {
            workspace: WorkspaceId(1),
            source: CastSource::Panel { id: "synthetic".into() },
            receiver: None,
            orientation: CastOrientation::Landscape,
            resolution: CastResolution::default(),
            pin: zeroize::Zeroizing::new(String::new()),
        };
        let snapshot = CastOutcome {
            receivers: vec![CastReceiver {
                id: "tv".into(),
                name: "Synthetic TV".into(),
            }],
            ..CastOutcome::default()
        };
        let mut parent = None;
        let mut popup: Option<egui::LayerId> = None;
        for _ in 0..4 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                ..egui::RawInput::default()
            };
            let _ = ctx
                .run_ui(input, |ui| {
                    let response = egui::Window::new("Cast")
                        .order(Order::Foreground)
                        .fixed_pos(Pos2::new(50.0, 50.0))
                        .show(ui.ctx(), |ui| {
                            if let Some(layer) = popup {
                                egui::Popup::open_id(ui.ctx(), layer.id);
                            }
                            let id = render_selection(ui, &mut picker, &snapshot, &mut None);
                            popup = Some(egui::LayerId::new(Order::Foreground, id.with("popup")));
                        })
                        .expect("window");
                    parent = Some(response.response.layer_id);
                    // Window interaction raises the parent even when the menu already exists.
                    ui.ctx().move_to_top(response.response.layer_id);
                })
                .discard_textures();
        }
        let popup = popup.expect("popup layer");
        let rect = ctx.memory(|memory| memory.area_rect(popup.id)).expect("menu rect");
        assert_ne!(Some(popup), parent);
        assert_eq!(ctx.layer_id_at(rect.center()), Some(popup));
    }
}

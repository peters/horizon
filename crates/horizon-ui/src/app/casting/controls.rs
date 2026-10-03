use super::{super::HorizonApp, Picker};
use crate::theme;
use egui::{Context, Id, Order, Pos2, Rect, Sense, Stroke, Vec2};
use horizon_core::browser::manifest::cast::{CastOperation, CastOrientation, CastResolution, CastSource};

impl HorizonApp {
    pub(super) fn render_cast_controls(&mut self, ctx: &Context) {
        if self.startup_chooser.is_some() || self.shutdown_progress.is_some() {
            return;
        }
        self.render_cast_icons(ctx);
        self.render_cast_notification(ctx);
        let Some(mut picker) = self.casting.picker.take() else {
            return;
        };
        let snapshot = self.cast_snapshot(picker.workspace, ctx);
        let mut open = true;
        let mut action = None;
        let mut close_after = false;
        let mut close_requested = false;
        let window = egui::Window::new("Cast")
            .id(Id::new("cast_picker"))
            .order(Order::Foreground)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .frame(
                egui::Frame::window(&ctx.global_style())
                    .inner_margin(14)
                    .corner_radius(12),
            )
            .min_width(264.0)
            .max_width(264.0)
            .default_width(264.0)
            .default_pos(egui::pos2(
                (self.canvas_rect(ctx).right() - 300.0).max(self.canvas_rect(ctx).left() + 12.0),
                self.canvas_rect(ctx).top() + 40.0,
            ))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                ui.spacing_mut().button_padding = egui::vec2(10.0, 6.0);
                render_picker_header(ui, &mut close_requested);
                ui.add_space(6.0);
                let menu_ids = render_selection(ui, &mut picker, &snapshot, &mut action);
                self.casting.control_menus =
                    Some(menu_ids.map(|id| egui::LayerId::new(Order::Foreground, id.with("popup"))));
                render_session_actions(
                    ui,
                    &mut picker,
                    &snapshot,
                    self.casting.pairing_loading(),
                    &mut action,
                    &mut close_after,
                );
                if let Some(notice) = &self.casting.notice {
                    ui.colored_label(ui.visuals().error_fg_color, notice);
                }
            });
        if let Some(window) = window
            && !egui::Popup::is_any_open(ctx)
        {
            ctx.move_to_top(window.response.layer_id);
        }
        if let Some(action) = action
            && !self.apply_cast_control(picker.workspace, &action, ctx)
        {
            close_after = false;
        }
        if open && !close_after && !close_requested {
            self.casting.picker = Some(picker);
        }
    }
    fn apply_cast_control(
        &mut self,
        workspace: horizon_core::WorkspaceId,
        action: &CastOperation,
        ctx: &Context,
    ) -> bool {
        let outcome = self.cast_operation(workspace, action, ctx);
        if let Some(error) = outcome.error {
            self.casting.notify(error);
            false
        } else {
            self.casting.notice = None;
            true
        }
    }
    fn render_cast_icons(&mut self, ctx: &Context) {
        if !self.host_dialog_open() && self.casting.picker.is_none() {
            for panel in &self.board.panels {
                let Some(&rect) = self.panel_screen_rects.get(&panel.id) else {
                    continue;
                };
                let id = panel.id;
                let workspace = panel.workspace_id;
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
                        if active {
                            response
                        } else {
                            response.on_hover_text("Cast panel or workspace")
                        }
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
                        source: session.map_or_else(
                            || CastSource::Panel {
                                id: panel.local_id.clone(),
                            },
                            |session| session.source.clone(),
                        ),
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

fn render_session_actions(
    ui: &mut egui::Ui,
    picker: &mut Picker,
    snapshot: &horizon_core::browser::manifest::cast::CastOutcome,
    pairing_loading: bool,
    action: &mut Option<CastOperation>,
    close_after: &mut bool,
) {
    let current = snapshot
        .sessions
        .iter()
        .find(|session| Some(&session.receiver_id) == picker.receiver.as_ref());
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);
    if let Some(session) = current {
        render_session_status(ui, session);
        if let Some(error) = &session.error {
            ui.add(egui::Label::new(egui::RichText::new(error).color(theme::PALETTE_RED())).wrap());
        }
        if session.encoder.is_some() {
            ui.collapsing("Details", |ui| render_encoder_status(ui, session));
        }
    }
    let busy = current.is_some_and(|session| !matches!(session.state.as_str(), "stopped" | "failed"));
    if let Some(session) = current.filter(|session| session.state == "pin_required") {
        ui.label(
            egui::RichText::new("Enter the code shown on your TV")
                .size(12.0)
                .color(theme::FG_SOFT()),
        );
        ui.add_sized(
            [ui.available_width(), 32.0],
            egui::TextEdit::singleline(&mut *picker.pin)
                .password(true)
                .char_limit(4)
                .hint_text("TV code"),
        );
        if primary_button(
            ui,
            "Pair and cast",
            picker.pin.len() == 4 && picker.pin.bytes().all(|byte| byte.is_ascii_digit()),
        )
        .clicked()
        {
            *action = Some(CastOperation::Pair {
                receiver_id: session.receiver_id.clone(),
                pin: picker.pin.to_string(),
            });
            *close_after = true;
        }
        if ui.small_button("Cancel pairing").clicked() {
            *action = Some(CastOperation::Stop {
                receiver_id: session.receiver_id.clone(),
            });
        }
    } else if busy {
        if let Some(session) = current
            && ui
                .add_sized([ui.available_width(), 34.0], egui::Button::new("Stop casting"))
                .clicked()
        {
            *action = Some(CastOperation::Stop {
                receiver_id: session.receiver_id.clone(),
            });
        }
    } else {
        let ready = !pairing_loading
            && snapshot
                .receivers
                .iter()
                .any(|receiver| Some(&receiver.id) == picker.receiver.as_ref());
        if primary_button(ui, "Start casting", ready).clicked()
            && let Some(receiver) = &picker.receiver
        {
            picker.pin = zeroize::Zeroizing::new(String::new());
            *close_after = snapshot.paired_receivers.iter().any(|paired| &paired.id == receiver);
            *action = Some(CastOperation::Start {
                receiver_id: receiver.clone(),
                source: picker.source.clone(),
                orientation: picker.orientation,
                resolution: picker.resolution,
            });
        }
    }
}

fn render_picker_header(ui: &mut egui::Ui, close: &mut bool) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
        paint_cast_icon(ui.painter(), rect, theme::ACCENT());
        ui.label(egui::RichText::new("Cast").size(16.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let response = ui.add(egui::Button::new("×").frame(false));
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close cast controls"));
            if response.clicked() {
                *close = true;
            }
        });
    });
}

fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let button = egui::Button::new((
        egui::Atom::grow(),
        egui::RichText::new(text).strong().color(theme::BG()),
        egui::Atom::grow(),
    ))
    .fill(theme::ACCENT())
    .min_size(egui::vec2(ui.available_width(), 34.0));
    ui.add_enabled(enabled, button)
}

fn render_session_status(ui: &mut egui::Ui, session: &horizon_core::browser::manifest::cast::CastSessionInfo) {
    let (text, color) = match session.state.as_str() {
        "streaming" => ("Casting", theme::PALETTE_GREEN()),
        "pin_required" => ("Pair your TV", theme::ACCENT()),
        "connecting" => ("Connecting…", theme::ACCENT()),
        "starting" => ("Starting…", theme::ACCENT()),
        "stopping" => ("Stopping…", theme::FG_SOFT()),
        "stopped" => ("Ready to cast", theme::FG_SOFT()),
        _ => ("Could not cast", theme::PALETTE_RED()),
    };
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.0, color);
        ui.label(egui::RichText::new(text).color(color).size(12.0));
    });
    if session.state == "streaming" {
        ui.add(
            egui::Label::new(
                egui::RichText::new("Picture paused while this menu is open.")
                    .size(11.0)
                    .color(theme::FG_SOFT()),
            )
            .wrap(),
        );
    }
}

fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(11.0).color(theme::FG_SOFT()));
}

fn segments<T: Copy + PartialEq>(ui: &mut egui::Ui, value: &mut T, choices: &[(T, &str)]) {
    let count: f32 = choices.iter().map(|_| 1.0_f32).sum();
    let width = (ui.available_width() - (count - 1.0) * 4.0) / count;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for &(choice, label) in choices {
            if ui
                .add_sized(
                    [width, 28.0],
                    egui::Button::new(label).selected(*value == choice).corner_radius(6),
                )
                .clicked()
            {
                *value = choice;
            }
        }
    });
}

fn render_selection(
    ui: &mut egui::Ui,
    picker: &mut Picker,
    snapshot: &horizon_core::browser::manifest::cast::CastOutcome,
    action: &mut Option<CastOperation>,
) -> [Id; 2] {
    let current = snapshot
        .sessions
        .iter()
        .find(|session| Some(&session.receiver_id) == picker.receiver.as_ref());
    let busy = current.is_some_and(|session| !matches!(session.state.as_str(), "stopped" | "failed"));
    if let Some(session) = current.filter(|_| busy) {
        picker.source = session.source.clone();
        picker.orientation = session.orientation;
        picker.resolution = session.resolution;
    }
    let parent_layer = ui.layer_id();
    field_label(ui, "Source");
    let source = ui
        .add_enabled_ui(!busy, |ui| {
            egui::ComboBox::from_id_salt("cast_source")
                .width(ui.available_width())
                .wrap_mode(egui::TextWrapMode::Truncate)
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
                })
        })
        .inner;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        field_label(ui, "Apple TV");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if snapshot.discovering {
                ui.add(egui::Spinner::new().size(12.0));
            } else if ui
                .add(egui::Button::new(egui::RichText::new("Refresh").size(11.0)).frame(false))
                .clicked()
            {
                *action = Some(CastOperation::Discover);
            }
        });
    });
    let previous_receiver = picker.receiver.clone();
    let receiver = egui::ComboBox::from_id_salt("cast_receiver")
        .width(ui.available_width())
        .wrap_mode(egui::TextWrapMode::Truncate)
        .selected_text(
            snapshot
                .receivers
                .iter()
                .chain(&snapshot.paired_receivers)
                .find(|receiver| Some(&receiver.id) == picker.receiver.as_ref())
                .map_or("Select Apple TV", |receiver| receiver.name.as_str()),
        )
        .show_ui(ui, |ui| {
            ui.ctx().set_sublayer(parent_layer, ui.layer_id());
            for receiver in &snapshot.receivers {
                ui.selectable_value(&mut picker.receiver, Some(receiver.id.clone()), &receiver.name);
            }
        });
    if previous_receiver != picker.receiver {
        picker.pin = zeroize::Zeroizing::new(String::new());
    }
    if snapshot.receivers.is_empty() && !snapshot.discovering {
        ui.add(
            egui::Label::new(
                egui::RichText::new("No TVs found. Check the network, then refresh.")
                    .size(11.0)
                    .color(theme::FG_SOFT()),
            )
            .wrap(),
        );
    }
    ui.add_space(4.0);
    ui.add_enabled_ui(!busy, |ui| {
        field_label(ui, "Quality");
        segments(
            ui,
            &mut picker.resolution,
            &[
                (CastResolution::Hd720, "720p"),
                (CastResolution::FullHd1080, "1080p"),
                (CastResolution::Uhd4k, "4K"),
            ],
        );
        segments(
            ui,
            &mut picker.orientation,
            &[
                (CastOrientation::Landscape, "Landscape"),
                (CastOrientation::Portrait, "Portrait"),
            ],
        );
    });
    [source.response.id, receiver.response.id]
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
                            popup = Some(egui::LayerId::new(Order::Foreground, id[1].with("popup")));
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

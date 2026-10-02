use std::sync::Arc;

use egui::{Button, FontId, RichText, Vec2, text::LayoutJob, text::TextFormat};
use horizon_core::{AgentSessionBinding, PanelId};

const PAGE_SIZE: usize = 8;

use super::session_deletion::{SessionDeletionUi, deletion_progress, queue_deletion_request};
use crate::{text::truncate_chars, theme};

#[derive(Clone)]
struct SessionPicker {
    panel_id: PanelId,
    last_rendered_frame: u64,
    focus_first: bool,
    options: Arc<[AgentSessionBinding]>,
    anchor: egui::Rect,
    layer: egui::LayerId,
    deletion: SessionDeletionUi,
}

pub(super) fn open_session_picker(response: &egui::Response, panel_id: PanelId, options: Vec<AgentSessionBinding>) {
    let id = picker_id(&response.ctx);
    let frame = response.ctx.cumulative_frame_nr();
    let deletion = SessionDeletionUi::restored(&response.ctx);
    response.ctx.data_mut(|data| {
        data.insert_temp(
            id,
            SessionPicker {
                panel_id,
                last_rendered_frame: frame,
                focus_first: true,
                options: options.into(),
                anchor: response.rect,
                layer: response.layer_id,
                deletion,
            },
        );
    });
}

pub(super) fn render_session_picker(ctx: &egui::Context, panel_id: PanelId) -> Option<AgentSessionBinding> {
    let id = picker_id(ctx);
    let mut state = ctx.data(|data| data.get_temp::<SessionPicker>(id))?;
    if state.panel_id != panel_id {
        return None;
    }
    state.last_rendered_frame = ctx.cumulative_frame_nr();
    let mut open = true;
    let result = egui::Popup::new(id, ctx.clone(), state.anchor, state.layer)
        .kind(egui::PopupKind::Tooltip)
        .info(egui::UiStackInfo::new(egui::UiKind::Menu))
        .open_bool(&mut open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .frame(
            egui::Frame::popup(&ctx.global_style())
                .corner_radius(16.0)
                .inner_margin(18.0)
                .stroke(egui::Stroke::new(1.0, theme::BORDER_STRONG())),
        )
        .show(|ui| render_options_with_deletion(ui, &state.options, state.focus_first, &mut state.deletion));
    state.focus_first = false;
    if open {
        ctx.data_mut(|data| data.insert_temp(id, state));
    } else {
        ctx.data_mut(|data| data.remove::<SessionPicker>(id));
    }
    result.and_then(|result| result.inner.binding)
}

pub(super) fn finish_session_deletion(
    ctx: &egui::Context,
    panel_id: PanelId,
    viewport: egui::ViewportId,
    options: Vec<AgentSessionBinding>,
    report: &horizon_core::AgentSessionDeletionReport,
) {
    let id = egui::Id::new(("session_recovery_picker", viewport));
    ctx.data_mut(|data| {
        if let Some(mut state) = data
            .get_temp::<SessionPicker>(id)
            .filter(|state| state.panel_id == panel_id)
        {
            state.options = options.into();
            state.focus_first = true;
            state.deletion.finish(report);
            data.insert_temp(id, state);
        }
    });
}

fn picker_id(ctx: &egui::Context) -> egui::Id {
    egui::Id::new(("session_recovery_picker", ctx.viewport_id()))
}

pub(in crate::app) fn session_picker_panel(ctx: &egui::Context) -> Option<PanelId> {
    let id = picker_id(ctx);
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|data| {
        let state = data.get_temp::<SessionPicker>(id)?;
        if frame > state.last_rendered_frame.saturating_add(1) {
            data.remove::<SessionPicker>(id);
            return None;
        }
        Some(state.panel_id)
    })
}

#[derive(Default)]
pub(super) struct SessionRebindRenderOutcome {
    pub(super) binding: Option<AgentSessionBinding>,
    #[cfg(test)]
    pub(super) option_rects: Vec<egui::Rect>,
    #[cfg(test)]
    pub(super) copy_rects: Vec<egui::Rect>,
}

#[cfg(test)]
pub(super) fn render_session_rebind_options(
    ui: &mut egui::Ui,
    rebind_options: &[AgentSessionBinding],
) -> SessionRebindRenderOutcome {
    render_options(ui, rebind_options, false)
}

#[cfg(test)]
fn render_options(
    ui: &mut egui::Ui,
    rebind_options: &[AgentSessionBinding],
    focus_first: bool,
) -> SessionRebindRenderOutcome {
    render_options_with_deletion(ui, rebind_options, focus_first, &mut SessionDeletionUi::default())
}

fn render_options_with_deletion(
    ui: &mut egui::Ui,
    rebind_options: &[AgentSessionBinding],
    focus_first: bool,
    deletion: &mut SessionDeletionUi,
) -> SessionRebindRenderOutcome {
    let page_id = ui.make_persistent_id("session_page");
    let reset_id = ui.make_persistent_id("session_page_reset");
    let reset_scroll = focus_first || ui.data(|data| data.get_temp::<bool>(reset_id).unwrap_or_default());
    let mut reset_next = false;
    let mut page = ui.data(|data| data.get_temp::<usize>(page_id).unwrap_or_default());
    page = if focus_first {
        0
    } else {
        page.min(rebind_options.len().saturating_sub(1) / PAGE_SIZE)
    };
    let mut outcome = SessionRebindRenderOutcome::default();
    let width = (ui.ctx().content_rect().width() - 40.0).clamp(280.0, 540.0);
    ui.set_width(width);
    ui.spacing_mut().button_padding = Vec2::new(12.0, 8.0);
    ui.spacing_mut().scroll.floating = false;
    ui.visuals_mut().widgets.inactive.weak_bg_fill = theme::PANEL_BG_ALT();
    let content_start = ui.cursor().top();
    render_session_header(ui, rebind_options.len());
    if let Some((done, total)) = deletion_progress(ui.ctx()) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(format!("Deleting saved conversations… {done}/{total}"));
        });
        return outcome;
    }
    if deletion.confirming() {
        if let Some(sessions) = deletion.render_confirmation(ui) {
            queue_deletion_request(ui.ctx(), sessions);
        }
        return outcome;
    }
    deletion.render_toolbar(ui, rebind_options);
    if rebind_options.is_empty() {
        ui.label("No saved conversations remain in this list.");
    }
    let mut scroll = egui::ScrollArea::vertical()
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .max_height(
            (ui.ctx().content_rect().height() - (ui.cursor().top() - content_start) - 160.0).clamp(100.0, 480.0),
        );
    if reset_scroll {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    scroll.show(ui, |ui| {
        for (index, binding) in rebind_options.iter().skip(page * PAGE_SIZE).take(PAGE_SIZE).enumerate() {
            render_session_row(ui, binding, focus_first && index == 0, deletion, &mut outcome);
            if outcome.binding.is_some() {
                break;
            }
            ui.add_space(8.0);
        }
    });
    reset_next |= render_session_pagination(ui, rebind_options.len(), &mut page);
    ui.data_mut(|data| {
        data.insert_temp(page_id, page);
        data.insert_temp(reset_id, reset_next);
    });
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(4.0);
    ui.label(
        RichText::new("Selecting a session restarts this panel.")
            .size(12.0)
            .color(theme::FG_SOFT()),
    );
    outcome
}

fn render_session_pagination(ui: &mut egui::Ui, count: usize, page: &mut usize) -> bool {
    let mut changed = false;
    if count > PAGE_SIZE {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(*page > 0, Button::new("Previous")).clicked() {
                *page -= 1;
                changed = true;
            }
            ui.label(format!(
                "{}–{} of {}",
                *page * PAGE_SIZE + 1,
                ((*page + 1) * PAGE_SIZE).min(count),
                count
            ));
            if ui
                .add_enabled((*page + 1) * PAGE_SIZE < count, Button::new("Next"))
                .clicked()
            {
                *page += 1;
                changed = true;
            }
        });
    }

    changed
}

fn render_session_header(ui: &mut egui::Ui, count: usize) {
    ui.horizontal(|ui| {
        egui::Frame::new()
            .fill(theme::alpha(theme::ACCENT(), 24))
            .corner_radius(12.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::hover());
                let center = rect.center();
                let stroke = egui::Stroke::new(1.8, theme::ACCENT());
                ui.painter().circle_stroke(center, 10.0, stroke);
                ui.painter()
                    .line_segment([center, center - Vec2::new(0.0, 6.0)], stroke);
                ui.painter()
                    .line_segment([center, center + Vec2::new(5.0, 3.0)], stroke);
            });
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.label(RichText::new("Resume a session").size(20.0).strong().color(theme::FG()));
            ui.label(
                RichText::new(format!(
                    "{} {} available · Newest first",
                    count,
                    if count == 1 { "session" } else { "sessions" }
                ))
                .size(12.0)
                .color(theme::FG_SOFT()),
            );
        });
    });
    ui.add_space(16.0);
}

fn render_session_row(
    ui: &mut egui::Ui,
    binding: &AgentSessionBinding,
    focus_first: bool,
    deletion: &mut SessionDeletionUi,
    outcome: &mut SessionRebindRenderOutcome,
) {
    let label = binding
        .label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| binding.kind.display_name());
    let mut job = LayoutJob::default();
    job.append(
        &truncate_chars(label, 60),
        0.0,
        TextFormat {
            font_id: FontId::proportional(15.0),
            line_height: Some(22.0),
            color: theme::FG(),
            ..Default::default()
        },
    );
    job.append(
        &format!("\n{}", binding.last_used_display()),
        0.0,
        TextFormat {
            font_id: FontId::proportional(12.0),
            line_height: Some(20.0),
            color: theme::FG_SOFT(),
            ..Default::default()
        },
    );
    job.append(
        &format!("\n{}", binding.session_id),
        0.0,
        TextFormat {
            font_id: FontId::monospace(12.0),
            line_height: Some(18.0),
            color: theme::FG_SOFT(),
            ..Default::default()
        },
    );
    ui.push_id((&binding.kind, &binding.session_id), |ui| {
        let mut focused = false;
        let card = egui::Frame::new()
            .fill(theme::PANEL_BG_ALT())
            .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE()))
            .corner_radius(12.0)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let response = ui.add_sized(
                        Vec2::new(ui.available_width() - 84.0, 78.0),
                        Button::new(job).right_text(()).frame(false).wrap(),
                    );
                    if focus_first {
                        response.request_focus();
                    }
                    focused = response.has_focus();
                    #[cfg(test)]
                    outcome.option_rects.push(response.rect);
                    if response.clicked() {
                        outcome.binding = Some(binding.clone());
                        ui.close();
                    }
                    response.on_hover_text(format!(
                        "{label}\nSession ID: {}\nClick to resume in this panel.",
                        binding.session_id
                    ));
                    ui.vertical(|ui| {
                        let copy = ui.add(
                            Button::new(RichText::new("Copy ID").size(12.0).color(theme::FG()))
                                .fill(theme::alpha(theme::ACCENT(), 20))
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(8.0),
                        );
                        focused |= copy.has_focus();
                        #[cfg(test)]
                        outcome.copy_rects.push(copy.rect);
                        if copy.clicked() {
                            ui.ctx().copy_text(binding.session_id.clone());
                        }
                        copy.on_hover_text("Copy the full session ID");
                        deletion.render_row_controls(ui, binding);
                    });
                })
            });
        if focused || card.response.contains_pointer() {
            ui.painter().rect_stroke(
                card.response.rect,
                12.0,
                egui::Stroke::new(1.0, theme::ACCENT()),
                egui::StrokeKind::Inside,
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;
    use egui::{Context, Event, FullOutput, Id, Key, Modifiers, PointerButton, Pos2, RawInput, Rect};
    use horizon_core::PanelKind;

    fn frame(ctx: &Context, events: Vec<Event>, launch: bool) -> FullOutput {
        let parent_id = Id::new("recovery_parent_menu");
        if launch {
            egui::Popup::open_id(ctx, parent_id);
        }
        ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 700.0))),
                events,
                ..RawInput::default()
            },
            |ui| {
                egui::Popup::new(
                    parent_id,
                    ctx.clone(),
                    Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::splat(10.0)),
                    ui.layer_id(),
                )
                .open_memory(None)
                .show(|ui| {
                    let response = ui.button("Resume a session…");
                    if launch {
                        open_session_picker(
                            &response,
                            PanelId(1),
                            vec![AgentSessionBinding::new(
                                PanelKind::Codex,
                                "exact-session-id".into(),
                                None,
                                Some("Recovery test".into()),
                                None,
                            )],
                        );
                        ui.close();
                    }
                });
                let binding = render_session_picker(ctx, PanelId(1));
                assert!(binding.is_none());
            },
        )
        .discard_textures()
    }

    fn text_center(output: &FullOutput, text: &str) -> Option<Pos2> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(shape) if shape.galley.job.text == text => {
                Some(shape.pos + shape.galley.size() / 2.0)
            }
            _ => None,
        })
    }

    #[test]
    fn panel_rendering_keeps_the_picker_visible_while_its_body_is_suppressed() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let ctx = Context::default();
        let workspace = app.board.create_workspace("recovery");
        let panel = app
            .board
            .create_panel(
                horizon_core::PanelOptions {
                    kind: PanelKind::Editor,
                    ..Default::default()
                },
                workspace,
            )
            .expect("editor panel");
        for frame_index in 0..3 {
            let output = ctx
                .run_ui(
                    RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0))),
                        ..Default::default()
                    },
                    |ui| {
                        if frame_index == 0 {
                            let response = ui.button("Launch picker");
                            open_session_picker(
                                &response,
                                panel,
                                vec![AgentSessionBinding::new(
                                    PanelKind::Codex,
                                    "exact-session-id".into(),
                                    None,
                                    Some("Recovery test".into()),
                                    None,
                                )],
                            );
                        }
                        app.handle_canvas_pan(&ctx);
                        app.render_panels(ui);
                    },
                )
                .discard_textures();
            if frame_index > 0 {
                assert!(
                    text_center(&output, "Resume a session").is_some(),
                    "picker visible in panel frame {frame_index}"
                );
            }
        }
    }

    #[test]
    fn count_and_paging_include_sessions_beyond_the_first_eight() {
        let ctx = Context::default();
        let options: Vec<_> = (1..=16)
            .map(|index| {
                AgentSessionBinding::new(
                    PanelKind::Codex,
                    format!("session-{index}"),
                    None,
                    Some(format!("Conversation {index}")),
                    None,
                )
            })
            .collect();
        let run = |events| {
            let mut outcome = None;
            let output = ctx
                .run_ui(
                    RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 900.0))),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        outcome = Some(render_options(ui, &options, false));
                    },
                )
                .discard_textures();
            (output, outcome.expect("options rendered"))
        };
        let (output, outcome) = run(Vec::new());
        assert_eq!(outcome.option_rects.len(), 8);
        assert!(text_center(&output, "16 sessions available · Newest first").is_some());
        let first_top = outcome.option_rects[0].top();
        run(vec![
            Event::PointerMoved(Pos2::new(250.0, 350.0)),
            Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -600.0),
                phase: egui::TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ]);
        for _ in 0..20 {
            run(Vec::new());
        }
        let (output, scrolled) = run(Vec::new());
        assert!(scrolled.option_rects[0].top() < first_top - 20.0);
        let at = text_center(&output, "Next").expect("next page available");
        for pressed in [true, false] {
            run(vec![
                Event::PointerMoved(at),
                Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ]);
        }
        let (output, outcome) = run(Vec::new());
        assert_eq!(outcome.option_rects.len(), 8);
        assert!((outcome.option_rects[0].top() - first_top).abs() < 1.0);
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.job.text.contains("Conversation 9"))));
        assert!(text_center(&output, "9–16 of 16").is_some());
    }

    #[test]
    fn keyboard_copy_keeps_the_picker_open() {
        let ctx = Context::default();
        frame(&ctx, Vec::new(), true);
        frame(&ctx, Vec::new(), false);
        let key = |key| Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        frame(&ctx, vec![key(Key::Tab)], false);
        let copied = frame(&ctx, vec![key(Key::Enter)], false);
        assert!(
            copied
                .platform_output
                .commands
                .iter()
                .any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "exact-session-id"))
        );
        assert_eq!(session_picker_panel(&ctx), Some(PanelId(1)));
    }

    #[test]
    fn picker_releases_input_when_its_owner_stops_rendering() {
        let ctx = Context::default();
        frame(&ctx, Vec::new(), true);
        assert_eq!(session_picker_panel(&ctx), Some(PanelId(1)));
        for _ in 0..2 {
            let _ = ctx.run_ui(RawInput::default(), |_| {}).discard_textures();
        }
        assert!(session_picker_panel(&ctx).is_none());
    }

    #[test]
    fn recovery_picker_outlives_parent_menu_and_copy_keeps_it_open() {
        let ctx = Context::default();
        frame(&ctx, Vec::new(), true);
        let visible = frame(&ctx, Vec::new(), false);
        assert!(!egui::Popup::is_id_open(&ctx, Id::new("recovery_parent_menu")));
        assert!(text_center(&visible, "Resume a session").is_some());
        assert_eq!(session_picker_panel(&ctx), Some(PanelId(1)));
        let at = text_center(&visible, "Copy ID").expect("copy action visible after parent closed");
        frame(
            &ctx,
            vec![
                Event::PointerMoved(at),
                Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ],
            false,
        );
        let copied = frame(
            &ctx,
            vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
            false,
        );
        assert!(
            copied
                .platform_output
                .commands
                .iter()
                .any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "exact-session-id"))
        );
        assert!(text_center(&frame(&ctx, Vec::new(), false), "Resume a session").is_some());
        frame(
            &ctx,
            vec![Event::Key {
                key: Key::Escape,
                physical_key: Some(Key::Escape),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            false,
        );
        assert!(text_center(&frame(&ctx, Vec::new(), false), "Resume a session").is_none());
        assert!(session_picker_panel(&ctx).is_none());
    }

    #[test]
    fn picker_blocks_root_shortcuts_and_restores_them_after_expiry() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let ctx = Context::default();
        let workspace = app.board.create_workspace("recovery");
        let panel = app
            .board
            .create_panel(
                horizon_core::PanelOptions {
                    kind: PanelKind::Editor,
                    ..Default::default()
                },
                workspace,
            )
            .expect("editor panel");
        app.shortcuts.toggle_settings = horizon_core::ShortcutBinding::parse("F1").expect("shortcut");
        app.shortcuts.command_palette = horizon_core::ShortcutBinding::parse("F2").expect("shortcut");
        for key in [Key::F1, Key::F2] {
            let _ = ctx
                .run_ui(RawInput::default(), |ui| {
                    open_session_picker(&ui.button("Resume"), panel, Vec::new());
                })
                .discard_textures();
            let _ = ctx
                .run_ui(
                    RawInput {
                        events: vec![Event::Key {
                            key,
                            physical_key: Some(key),
                            pressed: true,
                            repeat: false,
                            modifiers: Modifiers::NONE,
                        }],
                        ..Default::default()
                    },
                    |_| {
                        app.process_frame_inputs(&ctx);
                    },
                )
                .discard_textures();
            assert!(app.settings.is_none());
            assert!(app.command_palette.is_none());
        }
        for _ in 0..2 {
            let _ = ctx.run_ui(RawInput::default(), |_| {}).discard_textures();
        }
        let _ = ctx
            .run_ui(
                RawInput {
                    events: vec![Event::Key {
                        key: Key::F1,
                        physical_key: Some(Key::F1),
                        pressed: true,
                        repeat: false,
                        modifiers: Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |_| {
                    app.process_frame_inputs(&ctx);
                },
            )
            .discard_textures();
        assert!(app.settings.is_some());
    }

    #[test]
    fn picker_pointer_input_bypasses_canvas_gesture_reservation() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let ctx = Context::default();
        let workspace = app.board.create_workspace("recovery");
        let panel = app
            .board
            .create_panel(
                horizon_core::PanelOptions {
                    kind: PanelKind::Editor,
                    ..Default::default()
                },
                workspace,
            )
            .expect("editor panel");
        app.root_viewport_stabilizer = None;
        let input = || RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))),
            ..Default::default()
        };
        let _ = ctx
            .run_ui(input(), |ui| {
                open_session_picker(&ui.button("Resume"), panel, Vec::new());
            })
            .discard_textures();
        let pos = Pos2::new(500.0, 400.0);
        let events = vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::CTRL,
            },
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::CTRL,
            },
        ];
        let mut raw = RawInput {
            events: events.clone(),
            ..input()
        };
        app.filter_canvas_gesture(&ctx, &mut raw);
        assert_eq!(raw.events, events);
        assert!(app.canvas_gesture.take_completed().is_none());
        for _ in 0..2 {
            let _ = ctx.run_ui(input(), |_| {}).discard_textures();
        }
        raw.events = events.clone();
        app.filter_canvas_gesture(&ctx, &mut raw);
        assert_ne!(raw.events, events, "canvas reservation resumes after picker expiry");
    }

    #[test]
    fn picker_clicks_do_not_focus_underlying_panels() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let ctx = Context::default();
        let workspace = app.board.create_workspace("recovery");
        let mut create = |position| {
            app.board
                .create_panel(
                    horizon_core::PanelOptions {
                        kind: PanelKind::Editor,
                        position: Some(position),
                        size: Some([300.0, 200.0]),
                        ..Default::default()
                    },
                    workspace,
                )
                .expect("editor panel")
        };
        let owner = create([20.0, 100.0]);
        let other = create([600.0, 100.0]);
        app.board.focus(owner);
        let input = || RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))),
            ..Default::default()
        };
        let _ = ctx
            .run_ui(input(), |ui| {
                open_session_picker(&ui.button("Resume"), owner, Vec::new());
            })
            .discard_textures();
        let pos = app
            .visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)
            .into_iter()
            .find(|(id, _)| *id == other)
            .expect("other panel geometry")
            .1
            .screen_rect
            .center();
        let click = || RawInput {
            events: vec![Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
            ..input()
        };
        let _ = ctx
            .run_ui(click(), |_| {
                app.process_frame_inputs(&ctx);
            })
            .discard_textures();
        assert_eq!(app.board.focused, Some(owner));
        for _ in 0..2 {
            let _ = ctx.run_ui(input(), |_| {}).discard_textures();
        }
        let _ = ctx
            .run_ui(click(), |_| {
                app.process_frame_inputs(&ctx);
            })
            .discard_textures();
        assert_eq!(app.board.focused, Some(other));
    }
}

use super::*;
use crate::test_egui::DiscardTextures;
use egui::{Context, Event, RawInput, Rect, Vec2};

struct Harness {
    ctx: Context,
    workspaces: Vec<Workspace>,
    chosen: Option<WorkspaceId>,
    blocked: Option<WorkspaceId>,
}

impl Harness {
    fn new() -> Self {
        Self {
            ctx: Context::default(),
            workspaces: ["Current", "YouPark", "YouPay v2", "Ålesund", "YouPark"]
                .into_iter()
                .enumerate()
                .map(|(index, name)| Workspace::new(WorkspaceId(index as u64), name.into(), index))
                .collect(),
            chosen: None,
            blocked: None,
        }
    }

    fn frame(&mut self, events: Vec<Event>, render: bool, blocked: Option<WorkspaceId>) -> egui::FullOutput {
        self.blocked = blocked;
        self.ctx
            .run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, 600.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let launcher = ui.button("Panel");
                    if render {
                        self.chosen =
                            render_destination_search(ui, &launcher, &self.workspaces, WorkspaceId(0), |workspace| {
                                Some(workspace.id) != blocked
                            });
                    }
                },
            )
            .discard_textures()
    }

    fn key(&mut self, key: Key) {
        self.frame(
            vec![Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            true,
            self.blocked,
        );
    }
}

#[test]
fn typing_filters_case_insensitively_and_enter_selects_a_stable_id() {
    let mut harness = Harness::new();
    harness.frame(Vec::new(), true, None);
    harness.frame(vec![Event::Text("  yOuPaY  ".into())], true, None);
    harness.key(Key::Enter);
    assert_eq!(harness.chosen, Some(WorkspaceId(2)));
}

#[test]
fn unicode_search_and_duplicate_names_keep_distinct_destinations() {
    let mut harness = Harness::new();
    harness.frame(Vec::new(), true, None);
    harness.frame(vec![Event::Text("ÅLE".into())], true, None);
    harness.key(Key::Enter);
    assert_eq!(harness.chosen, Some(WorkspaceId(3)));
    let mut harness = Harness::new();
    harness.frame(Vec::new(), true, None);
    harness.frame(vec![Event::Text("youpark".into())], true, None);
    harness.key(Key::ArrowDown);
    harness.key(Key::Enter);
    assert_eq!(harness.chosen, Some(WorkspaceId(4)));
}

#[test]
fn empty_results_and_current_workspace_never_move_a_panel() {
    for query in ["missing", "Current"] {
        let mut harness = Harness::new();
        harness.frame(Vec::new(), true, None);
        harness.frame(vec![Event::Text(query.into())], true, None);
        harness.key(Key::ArrowDown);
        harness.key(Key::Enter);
        assert_eq!(harness.chosen, None);
    }
}

#[test]
fn arrows_wrap_and_skip_disabled_destinations() {
    let mut harness = Harness::new();
    harness.frame(Vec::new(), true, Some(WorkspaceId(1)));
    harness.key(Key::ArrowUp);
    harness.key(Key::Enter);
    assert_eq!(harness.chosen, Some(WorkspaceId(4)));
}

#[test]
fn removed_selection_reconciles_before_enter() {
    let mut harness = Harness::new();
    harness.frame(Vec::new(), true, None);
    harness.key(Key::ArrowDown);
    harness.workspaces.retain(|workspace| workspace.id != WorkspaceId(2));
    harness.key(Key::Enter);
    assert_eq!(harness.chosen, Some(WorkspaceId(1)));
}

#[test]
fn reopening_resets_search_and_focus() {
    let mut harness = Harness::new();
    harness.frame(Vec::new(), true, None);
    harness.frame(vec![Event::Text("missing".into())], true, None);
    harness.frame(Vec::new(), false, None);
    harness.frame(Vec::new(), false, None);
    harness.frame(Vec::new(), true, None);
    harness.key(Key::Enter);
    assert_eq!(harness.chosen, Some(WorkspaceId(1)));
}

#[test]
fn many_workspaces_do_not_expand_the_picker_beyond_the_viewport() {
    let mut harness = Harness::new();
    for index in 5..105 {
        harness
            .workspaces
            .push(Workspace::new(WorkspaceId(index), format!("Workspace {index}"), 0));
    }
    for _ in 0..3 {
        let output = harness.frame(Vec::new(), true, None);
        let bottom = output
            .shapes
            .iter()
            .map(|shape| shape.shape.visual_bounding_rect().bottom())
            .fold(0.0, f32::max);
        assert!(bottom < 600.0, "bounded picker bottom: {bottom}");
    }
}

#[test]
fn disabled_selection_reconciles_before_enter() {
    let mut harness = Harness::new();
    harness.frame(Vec::new(), true, None);
    harness.key(Key::ArrowDown);
    harness.frame(Vec::new(), true, Some(WorkspaceId(2)));
    harness.key(Key::Enter);
    assert_eq!(harness.chosen, Some(WorkspaceId(1)));
}

#[test]
fn popup_keeps_search_clicks_open_and_cancels_without_moving() {
    let ctx = Context::default();
    let workspaces = [Workspace::new(WorkspaceId(1), "Destination".into(), 0)];
    let menu_id = std::cell::Cell::new(egui::Id::NULL);
    let mut chosen = None;
    let mut run = |events, open| {
        ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ui| {
                let launcher = ui.button("Panel");
                let mut popup = egui::Popup::context_menu(&launcher)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .at_position(egui::pos2(20.0, 50.0));
                menu_id.set(popup.get_id());
                if open {
                    popup = popup.open_memory(Some(egui::SetOpenCommand::Bool(true)));
                }
                popup.show(|ui| {
                    chosen = render_destination_search(ui, &launcher, &workspaces, WorkspaceId(0), |_| true);
                    ui.separator();
                    let _ = ui.button("New Workspace");
                });
            },
        )
        .discard_textures()
    };
    run(Vec::new(), true);
    let output = run(Vec::new(), false);
    let at = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) if text.galley.text().contains("Search workspaces") => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .expect("search field hint");
    for pressed in [true, false] {
        run(
            vec![
                Event::PointerMoved(at),
                Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
            false,
        );
    }
    assert!(
        egui::Popup::is_id_open(&ctx, menu_id.get()),
        "clicking the search field keeps the menu open"
    );
    run(
        vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
        false,
    );
    assert!(!egui::Popup::is_id_open(&ctx, menu_id.get()));
    run(Vec::new(), true);
    for pressed in [true, false] {
        run(
            vec![
                Event::PointerMoved(egui::pos2(700.0, 500.0)),
                Event::PointerButton {
                    pos: egui::pos2(700.0, 500.0),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
            false,
        );
    }
    assert!(!egui::Popup::is_id_open(&ctx, menu_id.get()));
    assert_eq!(chosen, None);
}

#[test]
fn sidebar_search_field_is_above_the_sidebar_and_accepts_pointer_input() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let current = app.board.create_workspace("Current");
    let panel = app
        .board
        .create_panel(
            horizon_core::PanelOptions {
                kind: horizon_core::PanelKind::Editor,
                name: Some("Movable".into()),
                ..Default::default()
            },
            current,
        )
        .expect("editor panel");
    let destination = app.board.create_workspace("Destination");
    app.board.focus(panel);
    app.sidebar_visible = true;
    let ctx = Context::default();
    let mut run = |events| {
        ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |_ui| app.render_sidebar(&ctx),
        )
        .discard_textures()
    };
    run(Vec::new());
    let output = run(Vec::new());
    let center_of = |output: &egui::FullOutput, label: &str| {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text().contains(label) => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("rendered label")
    };
    let at = center_of(&output, "Movable");
    for pressed in [true, false] {
        run(vec![
            Event::PointerMoved(at),
            Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            },
        ]);
    }
    let output = run(Vec::new());
    let at = center_of(&output, "Search workspaces");
    let layer = ctx.layer_id_at(at).expect("popup layer");
    assert_eq!(layer.order, egui::Order::Tooltip);
    assert_ne!(
        layer.id,
        egui::Id::new("sidebar"),
        "the search field must be above sidebar chrome"
    );
    for pressed in [true, false] {
        run(vec![
            Event::PointerMoved(at),
            Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            },
        ]);
    }
    run(vec![Event::Text("destination".into())]);
    run(vec![Event::Key {
        key: Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    assert_eq!(
        app.board.panel(panel).expect("panel survives").workspace_id,
        destination
    );
}

#[test]
fn menu_search_field_uses_an_inset_well_instead_of_the_selection_stroke() {
    use crate::theme;

    let mut harness = Harness::new();
    theme::apply(&harness.ctx, horizon_core::AppearanceTheme::Dark);
    let output = harness.frame(Vec::new(), true, None);
    let hint = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) if text.galley.text().contains("Search workspaces") => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .expect("search field hint");
    let well = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Rect(rect)
                if rect.fill == theme::BG() && rect.rect.contains(hint) && (rect.rect.height() - 32.0).abs() < 1.0 =>
            {
                Some(rect.clone())
            }
            _ => None,
        })
        .expect("menu search well");
    let focus = egui::Stroke::new(1.0, theme::alpha(theme::ACCENT(), 200));
    assert_eq!(well.corner_radius, egui::CornerRadius::same(8));
    assert_eq!(well.stroke, focus);
    assert_eq!(well.stroke_kind, egui::StrokeKind::Inside);
    assert_ne!(
        well.stroke.color,
        theme::ACCENT(),
        "a focused menu search field must not paint the full-opacity selection ring"
    );
}

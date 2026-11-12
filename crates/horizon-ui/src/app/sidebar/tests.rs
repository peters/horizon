use super::{
    SidebarPanelEntry, SidebarWorkspaceInsert, sidebar_workspace_drop_should_dock, sidebar_workspace_insert_dock_side,
    sidebar_workspace_shows_panels,
};
use horizon_core::{WorkspaceDockSide, stand_in_program};

#[test]
fn sidebar_drop_docks_attached_workspace_against_attached_target() {
    assert!(sidebar_workspace_drop_should_dock(false));
}

#[test]
fn sidebar_drop_preserves_detached_workspace_reposition_against_attached_target() {
    assert!(sidebar_workspace_drop_should_dock(false));
}

#[test]
fn sidebar_drop_skips_board_docking_when_target_workspace_is_detached() {
    assert!(!sidebar_workspace_drop_should_dock(true));
}

#[test]
fn sidebar_insert_side_maps_to_expected_dock_side() {
    assert_eq!(
        sidebar_workspace_insert_dock_side(SidebarWorkspaceInsert::Before),
        WorkspaceDockSide::Left
    );
    assert_eq!(
        sidebar_workspace_insert_dock_side(SidebarWorkspaceInsert::After),
        WorkspaceDockSide::Right
    );
}

#[test]
fn sidebar_keeps_panels_expanded_when_accordion_is_disabled() {
    assert!(sidebar_workspace_shows_panels(false, false));
    assert!(sidebar_workspace_shows_panels(true, false));
}

#[test]
fn sidebar_hides_panels_for_inactive_workspaces_when_accordion_is_enabled() {
    assert!(!sidebar_workspace_shows_panels(false, true));
}

#[test]
fn sidebar_shows_panels_for_the_active_workspace_when_accordion_is_enabled() {
    assert!(sidebar_workspace_shows_panels(true, true));
}

// `is_focused` is decorative in these tests: the reveal helpers read the
// board's own focus state, not the sidebar entry flag.
fn sidebar_entry(id: horizon_core::PanelId, is_focused: bool) -> SidebarPanelEntry {
    SidebarPanelEntry {
        id,
        title: "panel".to_string(),
        label: "panel".to_string(),
        kind: horizon_core::PanelKind::Editor,
        is_focused,
        attention: horizon_core::Attention::Idle,
    }
}

fn editor_panel_options(name: &str) -> horizon_core::PanelOptions {
    horizon_core::PanelOptions {
        name: Some(name.to_string()),
        kind: horizon_core::PanelKind::Editor,
        command: Some("seed".to_string()),
        ..horizon_core::PanelOptions::default()
    }
}

#[test]
fn workspace_reveal_panel_prefers_the_focused_panel_of_the_workspace() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("a");
    let first = app
        .board
        .create_panel(editor_panel_options("first"), workspace)
        .expect("panel");
    let second = app
        .board
        .create_panel(editor_panel_options("second"), workspace)
        .expect("panel");
    app.board.focus(second);

    let panels = [sidebar_entry(first, false), sidebar_entry(second, true)];
    assert_eq!(app.workspace_reveal_panel(workspace, &panels), Some(second));
}

#[test]
fn workspace_reveal_panel_falls_back_to_first_panel() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("a");
    let other = app.board.create_workspace("b");
    let first = app
        .board
        .create_panel(editor_panel_options("first"), workspace)
        .expect("panel");
    let second = app
        .board
        .create_panel(editor_panel_options("second"), workspace)
        .expect("panel");
    let outsider = app
        .board
        .create_panel(editor_panel_options("other"), other)
        .expect("panel");

    // Focus lives in another workspace -> the workspace's first panel.
    app.board.focus(outsider);
    let panels = [sidebar_entry(first, false), sidebar_entry(second, false)];
    assert_eq!(app.workspace_reveal_panel(workspace, &panels), Some(first));

    // No focus at all -> first panel; no panels -> nothing to reveal.
    app.board.focused = None;
    assert_eq!(app.workspace_reveal_panel(workspace, &panels), Some(first));
    assert_eq!(app.workspace_reveal_panel(other, &[]), None);
}

#[test]
fn workspace_row_reveal_gate_covers_accordion_and_panel_count() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("a");
    let first = app
        .board
        .create_panel(editor_panel_options("first"), workspace)
        .expect("panel");
    let second = app
        .board
        .create_panel(editor_panel_options("second"), workspace)
        .expect("panel");
    let solo = app.board.create_workspace("solo");
    let only = app
        .board
        .create_panel(editor_panel_options("only"), solo)
        .expect("panel");
    let empty = app.board.create_workspace("empty");

    // Creation leaves focus on the last created panel (`only`), so put
    // it back in `workspace` for the focused-in-workspace case.
    app.board.focus(second);
    let two = [sidebar_entry(first, false), sidebar_entry(second, true)];
    let one = [sidebar_entry(only, false)];

    // Accordion rows always reveal a panel: focused-in-workspace for
    // multi-panel, the only panel for single-panel, nothing for empty.
    assert_eq!(app.workspace_row_reveal(true, workspace, &two), Some(second));
    assert_eq!(app.workspace_row_reveal(true, solo, &one), Some(only));
    assert_eq!(app.workspace_row_reveal(true, empty, &[]), None);

    // Flat rows only reveal single-panel workspaces; multi-panel rows
    // keep panning to the workspace bounds (None).
    assert_eq!(app.workspace_row_reveal(false, solo, &one), Some(only));
    assert_eq!(app.workspace_row_reveal(false, workspace, &two), None);
}

fn workspace_entry(row: horizon_core::workspace_list::Row, detached: bool) -> super::WorkspaceSidebarEntry {
    super::WorkspaceSidebarEntry {
        id: horizon_core::WorkspaceId(7),
        name: "sample".to_owned(),
        place: horizon_core::WorkspacePlace::Local,
        machine_panels: 0,
        color: egui::Color32::WHITE,
        is_active: false,
        detached,
        capabilities: crate::app::workspace::WorkspaceLayoutCapabilities {
            can_arrange: true,
            can_detach: false,
            can_remove_empty: false,
        },
        panels: Vec::new(),
        row,
    }
}

#[test]
fn a_dragged_workspace_drops_only_on_a_docked_row() {
    use horizon_core::workspace_list::Row;
    let docked = workspace_entry(Row::local(horizon_core::Attention::Idle, None), false);
    assert!(super::list::accepts_drop(&docked));
    assert!(!super::list::accepts_drop(&workspace_entry(
        Row::local(horizon_core::Attention::Idle, None),
        true
    )));
}

#[test]
fn the_new_window_badge_gives_way_to_the_name_at_the_minimum_sidebar() {
    use super::rows::name_right;
    // The minimum sidebar is 168 wide: the name starts at 64 and the first line ends at 150.
    let (right, badge) = name_right(64.0, 150.0, Some(7.0), Some(58.0));
    assert!(!badge, "a narrow row keeps the room for the name");
    assert!((right - 135.0).abs() <= f32::EPSILON);
    // The default sidebar is 210 wide: the badge fits and the name keeps 49.
    let (right, badge) = name_right(64.0, 192.0, Some(7.0), Some(58.0));
    assert!(badge);
    assert!((right - 113.0).abs() <= f32::EPSILON);
    // Without a count or a badge the name runs to the right edge.
    assert_eq!(name_right(64.0, 192.0, None, None), (192.0, false));
}

#[test]
fn the_badge_ring_takes_the_color_of_the_card_in_every_state() {
    use super::rows::{card_fill, row_surface};
    use crate::theme;
    let accent = egui::Color32::from_rgb(137, 180, 250);
    assert_eq!(
        row_surface(card_fill(accent, false, false, false)),
        theme::BG_ELEVATED()
    );
    assert_eq!(
        row_surface(card_fill(accent, true, false, false)),
        theme::PANEL_BG_ALT()
    );
    let hovered = row_surface(card_fill(accent, false, true, false));
    assert_eq!(
        hovered,
        theme::composite_over(theme::BG_ELEVATED(), theme::alpha(theme::PANEL_BG_ALT(), 170))
    );
    assert_ne!(hovered, theme::BG_ELEVATED());
    assert_eq!(
        row_surface(card_fill(accent, true, true, true)),
        theme::blend(theme::PANEL_BG_ALT(), accent, 0.22)
    );
}

#[test]
fn keyboard_focus_on_a_panel_row_shows_its_close_button() {
    use crate::test_egui::DiscardTextures;
    use egui::epaint::Shape;

    let (_temp, mut app) = crate::app::test_support::test_app();
    app.template_config.features.sidebar_accordion = true;
    let workspace = app.board.create_workspace("Quiet zone");
    let first = app
        .board
        .create_panel(editor_panel_options("first"), workspace)
        .expect("panel");
    let second = app
        .board
        .create_panel(editor_panel_options("second"), workspace)
        .expect("panel");
    app.board.active_workspace = Some(workspace);
    app.board.focus(first);
    let ctx = egui::Context::default();
    let frame = |app: &mut crate::app::HorizonApp| {
        let input = crate::app::test_support::raw_input([1400.0, 900.0], None);
        ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures()
    };
    let crosses_on = |output: &egui::FullOutput, label: &str| {
        let texts: Vec<(String, egui::Pos2)> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                Shape::Text(text) => Some((
                    text.galley.job.text.clone(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()).center(),
                )),
                _ => None,
            })
            .collect();
        let row = texts
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, at)| *at)
            .expect(label);
        texts
            .iter()
            .any(|(text, at)| text == "\u{00D7}" && (at.y - row.y).abs() < 4.0)
    };
    // An area shows from its second frame on.
    frame(&mut app);
    assert!(
        !crosses_on(&frame(&mut app), "second"),
        "a resting row hides its close button"
    );

    ctx.memory_mut(|memory| memory.request_focus(super::sidebar_panel_row_id(second)));
    frame(&mut app);
    assert!(
        crosses_on(&frame(&mut app), "second"),
        "a keyboard-focused row shows it"
    );
}

#[test]
fn the_snapshot_labels_only_the_panel_rows_that_show() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let shown = app.board.create_workspace("shown");
    let hidden = app.board.create_workspace("hidden");
    app.board
        .create_panel(editor_panel_options("first"), shown)
        .expect("panel");
    app.board
        .create_panel(editor_panel_options("second"), hidden)
        .expect("panel");
    app.board.active_workspace = Some(shown);
    let labels = |app: &crate::app::HorizonApp, workspace| {
        app.sidebar_workspace_data()
            .into_iter()
            .find(|entry| entry.id == workspace)
            .expect("workspace")
            .panels
            .into_iter()
            .map(|panel| panel.label)
            .collect::<Vec<_>>()
    };

    app.template_config.features.sidebar_accordion = true;
    assert_eq!(labels(&app, shown), ["first"]);
    assert_eq!(labels(&app, hidden), [""], "a collapsed workspace builds no labels");

    app.template_config.features.sidebar_accordion = false;
    assert_eq!(labels(&app, hidden), ["second"], "flat rows all show");
}

#[test]
fn a_workspace_that_lives_on_a_machine_says_so_on_its_status_line() {
    use crate::test_egui::DiscardTextures;
    use egui::epaint::Shape;

    let (_temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("Build");
    let local = app.board.create_workspace("Here");
    app.board
        .set_workspace_place(workspace, horizon_core::WorkspacePlace::Machine("build-1".to_owned()));
    let ctx = egui::Context::default();
    let render = |app: &mut crate::app::HorizonApp| {
        let input = crate::app::test_support::raw_input([1400.0, 900.0], None);
        ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures()
    };
    // An area shows from its second frame on.
    let _ = render(&mut app);
    let output = render(&mut app);
    let texts = |output: &egui::FullOutput| {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                Shape::Text(text) => Some((
                    text.galley.job.text.clone(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let painted = texts(&output);
    let name = painted
        .iter()
        .find(|(text, _)| text == "Build")
        .expect("the name of the workspace");
    let line = painted
        .iter()
        .find(|(text, _)| text == "on build-1")
        .expect("the status line of the placed workspace names its machine");
    assert!(
        line.1.min.y >= name.1.max.y,
        "the machine stands on the status line under the name, not after the name"
    );
    let entry = |app: &crate::app::HorizonApp, id| {
        app.sidebar_workspace_data()
            .into_iter()
            .find(|entry| entry.id == id)
            .expect("the workspace row")
    };
    assert_eq!(
        entry(&app, workspace).place.machine(),
        Some("build-1"),
        "the row reads the place of the workspace"
    );
    assert_eq!(entry(&app, local).place.machine(), None);
    assert_eq!(entry(&app, workspace).machine_panels, 0, "no panel holds a session yet");
}

#[test]
fn the_row_says_which_panels_hold_a_session_of_the_machine() {
    use horizon_core::{Machine, MachineRegistry, Machines, PanelOptions, WorkspacePlace};

    let (_temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("work");
    app.board
        .workspaces
        .iter_mut()
        .find(|candidate| candidate.id == workspace)
        .expect("the workspace")
        .local_id = "c8dfe3af-0e07-4889-99ed-403b4a5a4c8a".to_owned();
    app.board
        .set_workspace_place(workspace, WorkspacePlace::Machine("build-1".to_owned()));
    app.machine_list = Machines::with_records(
        MachineRegistry::from_home(app.session_store.home()),
        vec![Machine {
            name: "build-1".to_owned(),
            host: "203.0.113.10".to_owned(),
            ..Machine::default()
        }],
    );
    // The panel runs the stand-in program in place of ssh, so the test starts no ssh.
    let options = PanelOptions {
        launch_wrapper: vec![stand_in_program().to_owned()],
        ..PanelOptions::default()
    };
    app.create_panel_with_options(options, workspace)
        .expect("a terminal of the machine");
    let entry = app
        .sidebar_workspace_data()
        .into_iter()
        .find(|entry| entry.id == workspace)
        .expect("the workspace row");
    assert_eq!(entry.machine_panels, 1, "the row knows that a close ends one session");
}

#[test]
fn an_empty_workspace_is_removed_from_its_row_and_the_last_one_stays() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let lone = app.board.create_workspace("lone");
    let rows = app.sidebar_workspace_data();
    assert!(
        !rows
            .iter()
            .find(|row| row.id == lone)
            .unwrap()
            .capabilities
            .can_remove_empty,
        "the board keeps one workspace"
    );

    let empty = app.board.create_workspace("empty");
    let with_panel = app.board.create_workspace("with a panel");
    app.board
        .create_panel(editor_panel_options("a panel"), with_panel)
        .unwrap();
    let rows = app.sidebar_workspace_data();
    let capability = |id| {
        rows.iter()
            .find(|row| row.id == id)
            .unwrap()
            .capabilities
            .can_remove_empty
    };
    assert!(capability(empty), "an empty workspace can go");
    assert!(!capability(with_panel), "a workspace with a panel stays");

    app.apply_sidebar_actions(
        &egui::Context::default(),
        &super::SidebarActions {
            remove_empty_workspace: Some(empty),
            ..Default::default()
        },
    );
    assert!(app.board.workspace(empty).is_none(), "the row's action removed it");
    assert!(app.board.workspace(with_panel).is_some());
    assert!(app.board.workspace(lone).is_some());
}

/// The notice of a machine without a key names the machine, so it must reach the
/// person whole: the sidebar is narrow and one cut line hides the name.
#[test]
fn the_notice_of_the_machine_section_wraps_so_its_name_is_readable() {
    use crate::test_egui::DiscardTextures;
    use egui::epaint::Shape;

    let (_temp, mut app) = crate::app::test_support::test_app();
    let notice = "This computer has no key for build-a-long-machine-name yet";
    app.set_sidebar_notice(notice.to_owned());
    let ctx = egui::Context::default();
    let frame = |app: &mut crate::app::HorizonApp| {
        let input = crate::app::test_support::raw_input([1200.0, 900.0], None);
        ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures()
    };
    // A panel shows from its second frame on.
    frame(&mut app);
    let output = frame(&mut app);
    let galley = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == notice => Some(text.galley.clone()),
            _ => None,
        })
        .expect("the notice of the machine section is drawn");
    assert!(
        galley.rows.len() >= 2,
        "the notice wraps onto more than one line: {:?}",
        galley.rows
    );
    assert!(
        galley.rows.len() <= 4,
        "and it stays one short notice: {} rows",
        galley.rows.len()
    );
    let said: String = galley.rows.iter().map(|row| row.text()).collect::<Vec<_>>().join(" ");
    assert!(
        said.contains("build-a-long-machine-name"),
        "every word of the notice is shown: {said:?}"
    );
}

/// The tone of the notice decides its color, so an answer to a key press never
/// looks like a reason to act.
#[test]
fn the_notice_of_an_error_is_red_and_the_plain_answer_is_muted() {
    use crate::app::sidebar::machines::SidebarNoticeTone;
    use crate::test_egui::DiscardTextures;

    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = egui::Context::default();
    for (tone, colour) in [
        (SidebarNoticeTone::Error, crate::theme::PALETTE_RED()),
        (SidebarNoticeTone::Plain, crate::app::sidebar::text::secondary()),
    ] {
        let notice = format!("the notice of {tone:?}");
        match tone {
            SidebarNoticeTone::Error => app.set_sidebar_notice(notice.clone()),
            SidebarNoticeTone::Plain => app.set_sidebar_plain_notice(notice.clone()),
        }
        assert_eq!(app.sidebar_notice_tone, tone);
        // A panel shows from its second frame on.
        let mut drawn = None;
        for _ in 0..2 {
            let input = crate::app::test_support::raw_input([1200.0, 900.0], None);
            let output = ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures();
            drawn = output.shapes.iter().find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text == notice => Some(text.galley.clone()),
                _ => None,
            });
        }
        let galley = drawn.expect("the notice of the sidebar is drawn");
        assert_eq!(galley.job.sections[0].format.color, colour, "the tone of {tone:?}");
    }
}

/// The drawn texts of one frame of the sidebar.
fn sidebar_frame_texts(ctx: &egui::Context, app: &mut crate::app::HorizonApp) -> Vec<String> {
    use crate::test_egui::DiscardTextures;

    // A panel shows from its second frame on.
    for _ in 0..2 {
        let input = crate::app::test_support::raw_input([1200.0, 900.0], None);
        let output = ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures();
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_drawn_texts(&clipped.shape, &mut texts);
        }
        if !texts.is_empty() {
            return texts;
        }
    }
    Vec::new()
}

fn collect_drawn_texts(shape: &egui::Shape, texts: &mut Vec<String>) {
    match shape {
        egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_drawn_texts(shape, texts);
            }
        }
        _ => {}
    }
}

/// A pointer press and release at `pos`, in the frames that egui needs for a click.
fn press_at(ctx: &egui::Context, app: &mut crate::app::HorizonApp, pos: egui::Pos2) {
    use crate::app::test_support::run_app_frame_with_input;

    let pointer = |pressed: Option<bool>| {
        let mut input = crate::app::test_support::raw_input([1200.0, 900.0], Some([0.0, 0.0]));
        input.events.push(egui::Event::PointerMoved(pos));
        if let Some(pressed) = pressed {
            input.events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        input
    };
    run_app_frame_with_input(ctx, app, pointer(None));
    run_app_frame_with_input(ctx, app, pointer(Some(true)));
    run_app_frame_with_input(ctx, app, pointer(Some(false)));
}

/// The line about the panels that a session left out names something that is gone
/// for good, so it stays after the ten seconds of the short notice beside it, and
/// the short notice leaves on its own.
#[test]
fn the_line_about_left_out_panels_stays_after_a_short_notice_ends() {
    use std::time::Duration;

    let (_temp, mut app) = crate::app::test_support::test_app();
    let line = "This session had 2 panels of kinds that this Horizon removed, so they are left out: \
                Git Changes 1, Usage 1.";
    let reason = "build-a-machine: the terminal did not open";
    app.set_sidebar_sticky_notice(line.to_owned());
    app.set_sidebar_notice(reason.to_owned());
    let ctx = egui::Context::default();

    let texts = sidebar_frame_texts(&ctx, &mut app);
    assert!(
        texts
            .iter()
            .any(|drawn| drawn.contains("panels of kinds that this Horizon removed")),
        "the line that stays is drawn: {texts:?}"
    );
    assert!(
        texts.iter().any(|drawn| drawn.contains(reason)),
        "the short notice is drawn beside it: {texts:?}"
    );

    // The ten seconds of the short notice pass, which a test reads as a clock that
    // is ahead of the computer.
    app.notice_clock.ahead = super::machines::NOTICE + Duration::from_secs(1);
    let texts = sidebar_frame_texts(&ctx, &mut app);
    assert!(
        texts
            .iter()
            .any(|drawn| drawn.contains("panels of kinds that this Horizon removed")),
        "the line that stays is still there: {texts:?}"
    );
    assert!(
        !texts.iter().any(|drawn| drawn.contains(reason)),
        "the short notice ended on its own: {texts:?}"
    );
}

/// The line leaves when the person closes it, and only then.
#[test]
fn the_line_about_left_out_panels_leaves_on_its_close_control() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let line = "This session had 1 panel of kinds that this Horizon removed, so it is left out: Usage 1.";
    app.set_sidebar_sticky_notice(line.to_owned());
    let ctx = egui::Context::default();

    let texts = sidebar_frame_texts(&ctx, &mut app);
    assert!(
        texts.iter().any(|drawn| drawn.contains("so it is left out")),
        "the line is drawn: {texts:?}"
    );
    let control = ctx
        .read_response(super::machines::sticky_close_id())
        .expect("the close control of the line");
    press_at(&ctx, &mut app, control.rect.center());

    let texts = sidebar_frame_texts(&ctx, &mut app);
    assert!(
        !texts.iter().any(|drawn| drawn.contains("so it is left out")),
        "the close control ended the line: {texts:?}"
    );
    assert!(
        app.sidebar_sticky_notice.is_none(),
        "the line that stays is gone from the window, not only from the frame"
    );
}

/// A session that opened with panels of a kind that this Horizon removed sets the
/// line that stays, not the short notice of an action.
#[test]
fn a_session_that_left_panels_out_sets_the_line_that_stays() {
    use horizon_core::{Config, HorizonHome, RuntimeState, SessionOpenDisposition, SessionStore, StartupDecision};

    use crate::app::HorizonApp;

    // The board of an older session, read the way a start reads it.
    let board = "workspaces:\n- name: Work\n  panels:\n  - name: Usage\n    kind: usage\n  - name: Changes\n    \
                 kind: git_changes\n";
    let (_state, retired) = RuntimeState::from_yaml_reporting(board).expect("the board of an older session");
    let temp = tempfile::tempdir().expect("temp dir");
    let config_path = temp.path().join("config.json");
    let session_store = SessionStore::new(
        HorizonHome::from_root(temp.path().join(".horizon")),
        config_path.clone(),
    );
    let mut session = session_store
        .create_session_from_runtime(RuntimeState::default())
        .expect("a session of this home");
    session.retired_panels = retired;
    let ctx = egui::Context::default();
    let app = HorizonApp::new_with_egui_context(
        &ctx,
        &Config::default(),
        config_path,
        session_store,
        StartupDecision::Open {
            disposition: SessionOpenDisposition::Resume,
            session: Box::new(session),
        },
        crate::input::ObservedKeyboardInputs::default(),
    );

    assert_eq!(
        app.sidebar_sticky_notice.as_deref(),
        Some(
            "This session had 2 panels of kinds that this Horizon removed, so they are left out: \
             Git Changes 1, Usage 1."
        ),
        "the line that stays names what left the session"
    );
    assert!(
        app.sidebar_notice.is_none(),
        "the line that stays is not the short notice of an action"
    );
}

/// A keyboard reaches the close control, and Space presses it.
#[test]
fn the_close_control_of_the_line_answers_the_keyboard() {
    use crate::app::test_support::run_app_frame_with_input;

    let (_temp, mut app) = crate::app::test_support::test_app();
    let line = "This session had 1 panel of kinds that this Horizon removed, so it is left out: Usage 1.";
    app.set_sidebar_sticky_notice(line.to_owned());
    let ctx = egui::Context::default();
    sidebar_frame_texts(&ctx, &mut app);

    // The keyboard moves to the control, and Space presses what has the focus.
    ctx.memory_mut(|memory| memory.request_focus(super::machines::sticky_close_id()));
    let mut input = crate::app::test_support::raw_input([1200.0, 900.0], None);
    input.events.push(egui::Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    run_app_frame_with_input(&ctx, &mut app, input);

    assert!(
        app.sidebar_sticky_notice.is_none(),
        "Space on the focused close control ended the line"
    );
}

/// A setting that the server decided for another computer says so in the machine
/// section beside the state line of the server, and no timer ends that line: a
/// person who looks away while the link returns would never read it otherwise.
#[test]
fn the_line_about_a_setting_of_another_computer_stays_until_it_is_closed() {
    use std::time::Duration;

    let (_temp, mut app) = crate::app::test_support::test_app();
    let line = "The server kept the value of another computer for the setting appearance, so this computer takes \
                that value.";
    app.settings_lost = Some(line.to_owned());
    let ctx = egui::Context::default();

    let texts = sidebar_frame_texts(&ctx, &mut app);
    assert!(
        texts
            .iter()
            .any(|drawn| drawn.contains("kept the value of another computer")),
        "the line is drawn in the machine section: {texts:?}"
    );

    // The ten seconds of a short notice pass, and the line is still there.
    app.notice_clock.ahead = super::machines::NOTICE + Duration::from_secs(1);
    let texts = sidebar_frame_texts(&ctx, &mut app);
    assert!(
        texts
            .iter()
            .any(|drawn| drawn.contains("kept the value of another computer")),
        "no timer ends the line: {texts:?}"
    );

    let control = ctx
        .read_response(super::machines::settings_lost_close_id())
        .expect("the close control of the line");
    press_at(&ctx, &mut app, control.rect.center());

    assert!(
        app.settings_lost.is_none(),
        "the close control ends the line in the window, not only in the frame"
    );
    let texts = sidebar_frame_texts(&ctx, &mut app);
    assert!(
        !texts
            .iter()
            .any(|drawn| drawn.contains("kept the value of another computer")),
        "the line is gone: {texts:?}"
    );
}

/// The short notice of an action neither replaces the line about a setting of
/// another computer nor ends with it.
#[test]
fn a_short_notice_leaves_the_line_about_a_setting_of_another_computer() {
    use std::time::Duration;

    let (_temp, mut app) = crate::app::test_support::test_app();
    let line = "The server kept the value of another computer for the setting appearance, so this computer takes \
                that value.";
    app.settings_lost = Some(line.to_owned());
    app.set_sidebar_notice("build-a-machine: the terminal did not open".to_owned());
    let ctx = egui::Context::default();

    let texts = sidebar_frame_texts(&ctx, &mut app);
    assert!(
        texts
            .iter()
            .any(|drawn| drawn.contains("kept the value of another computer")),
        "the line of the setting is drawn: {texts:?}"
    );
    assert!(
        texts.iter().any(|drawn| drawn.contains("the terminal did not open")),
        "the short notice is drawn beside it: {texts:?}"
    );

    app.notice_clock.ahead = super::machines::NOTICE + Duration::from_secs(1);
    let texts = sidebar_frame_texts(&ctx, &mut app);
    assert!(
        texts
            .iter()
            .any(|drawn| drawn.contains("kept the value of another computer")),
        "the line of the setting stays after the short notice ends: {texts:?}"
    );
    assert!(
        !texts.iter().any(|drawn| drawn.contains("the terminal did not open")),
        "the short notice ended on its own: {texts:?}"
    );
}

#[test]
fn the_word_of_a_panel_state_fits_the_narrowest_sidebar() {
    use super::panel_rows::{STATE_GAP, title_and_state};
    // The narrowest sidebar is 168 wide. Its rows start 8 in, the card ends 8
    // before the edge, the title starts at 68 and the row keeps 26 for a close.
    let right = 160.0 - 26.0;
    let title_left = 68.0;
    // The longest word of a state at 10 pt Inter, and "Needs you".
    for state_width in [105.0, 46.0, 30.0] {
        let (title_right, state_left, width) =
            title_and_state(right, title_left, state_width).expect("a row with a state word");
        assert!(
            (state_left + width - right).abs() <= f32::EPSILON,
            "the word ends at the right edge of the row's text"
        );
        assert!(state_left + width <= 168.0, "the word stays inside the sidebar");
        assert!(width <= state_width, "a word that does not fit is cut, and never grows");
        assert!(title_right >= title_left, "the title keeps a room to elide in");
        if state_width < right - title_left {
            assert!(
                (title_right - (state_left - STATE_GAP)).abs() <= f32::EPSILON,
                "the title gives the room of the word away"
            );
        }
    }
    // Without a state word the title runs to the right edge.
    assert_eq!(title_and_state(right, title_left, 0.0), None);
}

#[test]
fn a_narrow_row_with_a_long_workspace_name_shows_the_agents_that_wait() {
    use egui::ViewportInfo;
    let (_temp, ctx, mut app) =
        crate::app::test_support::test_app_with_startup(horizon_core::StartupDecision::Ephemeral {
            runtime_state: Box::new(horizon_core::RuntimeState::default()),
        });
    let workspace = app
        .board
        .create_workspace("A very long workspace name that cannot fit the sidebar");
    let now = horizon_core::current_unix_millis();
    for (name, states) in [
        (
            "Fix the sync of the billing service",
            vec![horizon_core::agent_work::TurnState::Blocked],
        ),
        ("Write the docs", vec![horizon_core::agent_work::TurnState::Working]),
    ] {
        let panel = app
            .board
            .create_panel(
                horizon_core::PanelOptions {
                    name: Some(name.to_owned()),
                    kind: horizon_core::PanelKind::Claude,
                    command: Some("seed".to_owned()),
                    ..horizon_core::PanelOptions::default()
                },
                workspace,
            )
            .expect("panel");
        let local = app.board.panel(panel).expect("panel").local_id.clone();
        for (index, state) in states.into_iter().enumerate() {
            let at = now - 2 + i64::try_from(index).expect("index");
            app.apply_attention_values(
                vec![(panel, local.clone(), Some(attention_ledger(state, at)))],
                &egui::Context::default(),
            );
        }
    }
    app.board.focused = None;

    // The narrowest sidebar: a viewport of 360 makes the sidebar 168 wide.
    let mut input = egui::RawInput::default();
    input.viewports.insert(
        egui::ViewportId::ROOT,
        ViewportInfo {
            native_pixels_per_point: Some(1.0),
            inner_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 720.0))),
            ..ViewportInfo::default()
        },
    );
    crate::app::test_support::run_app_frame_with_input(&ctx, &mut app, input);

    let rows = app.read_sidebar_rows();
    let row = rows.get(&workspace).expect("row");
    assert_eq!(row.attention, horizon_core::Attention::NeedsYou);
    assert_eq!(
        row.waiting.line().as_deref(),
        Some("1 needs you"),
        "the count is the front of the line and survives the cut"
    );
}

fn attention_ledger(
    state: horizon_core::agent_work::TurnState,
    updated_at_millis: i64,
) -> horizon_core::agent_work::TurnLedger {
    let mut ledger = horizon_core::agent_work::TurnLedger::default();
    ledger.session_id = "session".into();
    ledger.prompt_id = Some("prompt".into());
    ledger.state = state;
    ledger.updated_at_millis = updated_at_millis;
    ledger
}

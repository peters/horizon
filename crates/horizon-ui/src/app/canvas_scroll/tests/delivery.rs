use super::*;

#[test]
fn an_absorbed_gesture_cannot_scroll_another_hovered_terminal() {
    let (_temp, ctx, mut app) = app_fixture();
    let workspace = app.board.panels[0].workspace_id;
    let owner = app.board.panels[0].id;
    let other = horizon_core::PanelId(99);
    let (_owner_history, mut first) = history_panel(owner, workspace);
    let (_other_history, mut second) = history_panel(other, workspace);
    first.layout.position = [0.0, 0.0];
    first.layout.size = [400.0, 350.0];
    second.layout.position = [450.0, 0.0];
    second.layout.size = first.layout.size;
    app.board.panels = vec![first, second];
    for time in [0.1, 0.2, 0.3] {
        let mut input = raw_input([1400.0, 900.0], None);
        input.time = Some(time);
        let _ = run_app_frame_with_input(&ctx, &mut app, input);
    }
    for panel in &mut app.board.panels {
        panel.set_scrollback(5);
    }
    let geometry = app.visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None);
    let body = |id| {
        geometry
            .iter()
            .find(|(panel, _)| *panel == id)
            .expect("panel")
            .1
            .terminal_body_screen_rect
            .expect("terminal body")
            .center()
    };
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        scroll_frame(1.0, body(owner), Vec2::ZERO, TouchPhase::Start),
    );
    let pan = app.canvas_view.pan_offset;
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        scroll_frame(1.016, body(other), Vec2::new(0.0, -20.0), TouchPhase::Move),
    );
    assert_offset(app.canvas_view.pan_offset, pan);
    for panel in &app.board.panels {
        assert_eq!(panel.terminal().expect("terminal").scrollback(), 5);
    }
    assert!(!ctx.input(|input| {
        input
            .events
            .iter()
            .any(|event| matches!(event, Event::MouseWheel { .. }))
    }));
    // A fresh contact over the second panel still reaches it normally.
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        scroll_frame(1.032, body(other), Vec2::new(0.0, -20.0), TouchPhase::Start),
    );
    assert!(app.board.panels[1].terminal().expect("terminal").scrollback() < 5);
}

#[test]
fn panel_delivery_defers_the_exact_canvas_tail_until_an_idle_frame() {
    let ctx = Context::default();
    let frame = |time, target, events| {
        let mut pan = Vec2::ZERO;
        let mut retained = Vec::new();
        let mut input = raw_input([1400.0, 900.0], None);
        input.time = Some(time);
        input.events = events;
        let _ = ctx
            .run_ui(input, |ui| {
                let ctx = ui.ctx();
                let mut routing = route_canvas_scroll(ctx, target, true, |_, _| false);
                routing.discard_displaced_wheels(target);
                routing.defer_for_panel_delivery(ctx);
                pan = routing.pan;
                routing.consume(ctx, &mut Vec::new());
                retained = ctx.input(|input| input.events.clone());
            })
            .discard_textures();
        (pan, retained)
    };
    assert_eq!(
        frame(
            1.0,
            ScrollTarget::Canvas,
            vec![wheel(Vec2::new(0.0, -3.0), TouchPhase::Start)]
        )
        .0,
        Vec2::new(0.0, -3.0)
    );
    let panel_start = wheel(Vec2::new(0.0, -5.0), TouchPhase::Start);
    let (pan, retained) = frame(
        1.016,
        ScrollTarget::Panel(PANEL),
        vec![wheel(Vec2::new(0.0, -7.0), TouchPhase::End), panel_start.clone()],
    );
    assert_eq!(pan, Vec2::ZERO);
    assert_eq!(retained, vec![panel_start]);
    for time in [1.032, 1.048] {
        let panel_move = wheel(Vec2::new(0.0, -5.0), TouchPhase::Move);
        let (pan, retained) = frame(time, ScrollTarget::Panel(PANEL), vec![panel_move.clone()]);
        assert_eq!(pan, Vec2::ZERO);
        assert_eq!(retained, vec![panel_move]);
    }
    assert_eq!(
        frame(1.064, ScrollTarget::Panel(PANEL), Vec::new()).0,
        Vec2::new(0.0, -7.0)
    );
    assert_eq!(frame(1.080, ScrollTarget::Panel(PANEL), Vec::new()).0, Vec2::ZERO);
    let _ = frame(
        1.1,
        ScrollTarget::Canvas,
        vec![wheel(Vec2::new(0.0, -3.0), TouchPhase::Start)],
    );
    let _ = frame(
        1.116,
        ScrollTarget::Panel(PANEL),
        vec![
            wheel(Vec2::new(0.0, -7.0), TouchPhase::End),
            wheel(Vec2::new(0.0, -5.0), TouchPhase::Start),
        ],
    );
    super::super::reset_canvas_scroll(&ctx);
    assert_eq!(frame(1.132, ScrollTarget::Panel(PANEL), Vec::new()).0, Vec2::ZERO);
}

#[test]
#[cfg(unix)] // Exercises a real PTY's raw mouse-report input using a Unix terminal.
fn a_new_terminal_app_gesture_receives_input_before_the_canvas_tail_moves_it() {
    use alacritty_terminal::term::TermMode;
    use horizon_core::{Panel, PanelOptions};
    use std::time::{Duration, Instant};

    let (_temp, ctx, mut app) = app_fixture();
    let output = tempfile::tempdir().expect("PTY receipt directory");
    let receipt = output.path().join("mouse-input");
    let command = "import os,sys,tty; tty.setraw(0); print('\\x1b[?1000h',end='',flush=True); data=os.read(0,6); open(sys.argv[1],'wb').write(data)";
    let mut terminal = Panel::spawn(
        app.board.panels[0].id,
        app.board.panels[0].workspace_id,
        PanelOptions {
            command: Some("/usr/bin/python3".into()),
            args: vec!["-c".into(), command.into(), receipt.to_string_lossy().into_owned()],
            ..PanelOptions::default()
        },
    )
    .expect("spawn mouse reporting terminal");
    terminal.layout = app.board.panels[0].layout;
    app.board.panels[0] = terminal;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.board.panels[0]
        .terminal()
        .expect("terminal")
        .mode()
        .contains(TermMode::MOUSE_REPORT_CLICK)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        app.board.panels[0]
            .terminal()
            .expect("terminal")
            .mode()
            .contains(TermMode::MOUSE_REPORT_CLICK)
    );
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        scroll_frame(1.0, Pos2::new(1350.0, 850.0), Vec2::new(0.0, -3.0), TouchPhase::Start),
    );
    let body = app.visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)[0]
        .1
        .terminal_body_screen_rect
        .expect("body");
    let pan = app.canvas_view.pan_offset;
    let mut input = scroll_frame(1.016, body.center(), Vec2::new(0.0, -7.0), TouchPhase::End);
    input.events.push(Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: Vec2::new(0.0, -1.0),
        phase: TouchPhase::Start,
        modifiers: Modifiers::NONE,
    });
    let _ = run_app_frame_with_input(&ctx, &mut app, input);
    assert_offset(app.canvas_view.pan_offset, pan);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !std::fs::read(&receipt).is_ok_and(|bytes| bytes.starts_with(b"\x1b[M")) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        std::fs::read(&receipt)
            .expect("terminal received wheel")
            .starts_with(b"\x1b[M")
    );
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        scroll_frame(1.032, body.center(), Vec2::ZERO, TouchPhase::Move),
    );
    assert_offset(app.canvas_view.pan_offset, [pan[0], pan[1] - 7.0]);
}

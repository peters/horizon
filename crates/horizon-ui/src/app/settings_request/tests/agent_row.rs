//! The state of the agent that asked for a server link, across the press on
//! `Link`: the write of the settings and the start of the link change no state
//! of an agent row.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use horizon_core::{Attention, PanelId, PanelOptions};

use super::*;
use crate::app::test_support::run_app_frame;

/// The stand-in program of a codex panel of the fixture.
const STAND_IN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/device-smoke/codex_attention_standin.py"
);
/// The hang guard of a wait for a word of the sidebar.
const PATIENCE: Duration = Duration::from_secs(120);

fn python3() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|folder| folder.join("python3"))
        .find(|program| program.is_file())
}

/// A codex panel of the stand-in beside the panels of `app`, whose program
/// starts with `arguments`.
fn codex_panel(
    app: &mut HorizonApp,
    folder: &std::path::Path,
    python: &std::path::Path,
    arguments: &[&str],
) -> PanelId {
    let workspace = app.board.panels[0].workspace_id;
    app.board
        .create_panel(
            PanelOptions {
                name: Some("Codex stand-in".to_owned()),
                kind: PanelKind::Codex,
                command: Some(python.display().to_string()),
                args: [STAND_IN, "--startup"]
                    .iter()
                    .chain(arguments)
                    .map(|argument| (*argument).to_owned())
                    .collect(),
                cwd: Some(folder.to_path_buf()),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("a codex panel of the stand-in")
}

/// Runs frames until the row of `local` shows `state`.
fn wait_for(ctx: &Context, app: &mut HorizonApp, local: &str, state: Attention) {
    let deadline = Instant::now() + PATIENCE;
    while app.panel_attention(local) != state {
        assert!(
            Instant::now() < deadline,
            "the row shows {state:?}, and it showed {:?}",
            app.panel_attention(local)
        );
        run_app_frame(ctx, app);
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Types one word into the panel, as the person does.
fn word(app: &mut HorizonApp, panel: PanelId, word: &str) {
    let panel = app.board.panel_mut(panel).expect("the panel of the test");
    assert!(panel.write_input(format!("{word}\n").as_bytes()));
}

#[test]
fn the_row_of_the_agent_that_asked_keeps_its_state_across_a_confirmed_link() {
    let Some(python) = python3() else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    // The titles of the program alone, and the titles with the reports of the
    // program status protocol. The card has the keyboard of the person at the
    // press, so the press gives the keyboard back to the agent that asked.
    for arguments in [&[][..], &["--protocol"][..]] {
        let (temp, ctx, mut app) = app();
        let agent = codex_panel(&mut app, temp.path(), &python, arguments);
        let local = app.board.panel(agent).expect("the agent").local_id.clone();
        // The stand-in spins while it starts, and its plain title says that it
        // is ready for a word. That title stands until the turn writes over it,
        // so a frame after it reads it for the row.
        let deadline = Instant::now() + PATIENCE;
        loop {
            run_app_frame(&ctx, &mut app);
            let panel = app.board.panel(agent).expect("the agent");
            if !panel.terminal_title.is_empty() && panel.title_state() == horizon_core::agent_work::TitleState::Plain {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the stand-in writes the title of a ready program"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        run_app_frame(&ctx, &mut app);
        app.board.focus(agent);
        word(&mut app, agent, "start");
        wait_for(&ctx, &mut app, &local, Attention::Working);

        // The agent asks in the middle of its turn, and the person presses Link.
        let mut asked = request(&app, link("server-vm"));
        asked.actor = format!("horizon:{local}");
        assert!(app.apply_block_request_waiting(&asked, &ctx).is_none());
        run_app_frame(&ctx, &mut app);
        let card = asking_card(&mut app);
        let card = app.board.panel_id_by_local_id(&card).expect("the card of the request");
        app.board.focus(card);
        app.press_host_card_button(&ctx, card, APPLY);
        assert_eq!(app.board.focused, Some(agent), "the agent that asked has the keyboard");

        // The frames of the link: the settings are written, the link starts and
        // answers, and the machine list is read again.
        let pressed = Instant::now();
        let mut answered = None;
        loop {
            run_app_frame(&ctx, &mut app);
            app.settle_browser_host_io();
            assert_eq!(
                app.panel_attention(&local),
                Attention::Working,
                "the row of the agent keeps its word {:?} after the press, with {arguments:?}",
                pressed.elapsed()
            );
            if answered.is_none() && !app.settings_request.is_waiting() {
                answered = Some(Instant::now());
            }
            if answered.is_some_and(|at| at.elapsed() >= Duration::from_millis(800)) {
                break;
            }
            assert!(pressed.elapsed() < PATIENCE, "the link of the confirmed entry answers");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            app.template_config.server.names_server(),
            "the press wrote the entry of the server"
        );

        // The end of the turn is unseen while the person looks elsewhere.
        app.board.focused = None;
        word(&mut app, agent, "stop");
        wait_for(&ctx, &mut app, &local, Attention::Done);
    }
}

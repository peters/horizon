//! The banner of the sessions that wait for a review, at the smallest window:
//! one row for each session, with its control on the row.
//!
//! A session waits for a review when Horizon restores an agent panel that
//! continues its work and that started with the command of its kind. The lane
//! therefore runs in a process of its own, whose `PATH` holds a stand-in of that
//! command and whose home is a folder of the test.

use egui::Event;
use horizon_core::{PanelKind, PanelState};

use super::{SCREEN_LINE, Window, texts};
use crate::app::test_support::editor_workspace_state;

/// The variable that marks the process of the lane.
const CHILD: &str = "HORIZON_TEST_RESUME_BANNER_CHILD";
/// The name of the lane, as the test binary selects it.
const LANE: &str =
    "app::small_window_tests::resume_banner::the_banner_of_a_small_window_keeps_one_row_for_each_session";
/// The sentence of a session that Horizon restored without evidence.
const SENTENCE: &str = "Automatic continuation is not verified for this session.";
const NAME: &str = "Atlas checkout redesign lane";

#[test]
fn the_banner_of_a_small_window_keeps_one_row_for_each_session() {
    if std::env::var_os(CHILD).is_some() {
        banner_lane();
        return;
    }
    let home = tempfile::tempdir().expect("the home of the lane");
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).expect("the folder of the stand-in");
    std::fs::write(home.path().join(".bashrc"), "").expect("the shell settings of the lane");
    horizon_core::stand_in_program_in(&bin, "claude", super::AGENT_PROGRAM).expect("the stand-in of the agent");
    let output = std::process::Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", LANE, "--nocapture"])
        .env_clear()
        .env(CHILD, "1")
        .env("HOME", home.path())
        .env("TMPDIR", std::env::temp_dir())
        .env("SHELL", "/bin/bash")
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HORIZON_AGENT_INTEGRATIONS", "off")
        .current_dir(home.path())
        .output()
        .expect("the process of the lane");
    assert!(
        output.status.success(),
        "the lane failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A window of 800 by 600 points whose board Horizon restored with one agent
/// panel that continues its work.
fn window_with_a_waiting_session() -> Window {
    let home = std::env::var("HOME").expect("the home of the lane");
    let mut workspace = editor_workspace_state("lane", [40.0, 40.0]);
    workspace.panels = vec![PanelState {
        local_id: "5f0c2a58-6f9d-4d3b-9a44-0c6c1f2b7e11".to_owned(),
        name: NAME.to_owned(),
        name_is_custom: Some(true),
        kind: PanelKind::Claude,
        cwd: Some(home),
        work_resume: horizon_core::agent_work::ResumePolicy {
            enabled: true,
            ..horizon_core::agent_work::ResumePolicy::default()
        },
        rows: 24,
        cols: 80,
        position: Some([60.0, 100.0]),
        size: Some([520.0, 340.0]),
        ..PanelState::default()
    }];
    let mut window = Window::new([800.0, 600.0], vec![workspace]);
    // The board of a restored agent stands when its start answered, and the
    // stand-in prints its line when it runs.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let _ = window.frame(Vec::new());
        let screen = window
            .app
            .board
            .panels
            .first()
            .and_then(horizon_core::Panel::terminal)
            .map_or_else(String::new, |terminal| terminal.last_lines_text(20));
        if screen.contains(SCREEN_LINE) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the stand-in of the restored agent runs: {screen:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window
}

fn banner_lane() {
    let mut window = window_with_a_waiting_session();
    let _ = window.frame(Vec::new());
    let drawn = texts(&window.frame(Vec::new()));
    let ctx = window.ctx.clone();
    let zone = window
        .app
        .work_resume_overlay_rect(&ctx)
        .unwrap_or_else(|| panic!("the restored session waits for a review: {drawn:?}"));
    // The banner stands at the top of the canvas, above the panel of the session.
    let find = |text: &str| {
        drawn
            .iter()
            .filter(|entry| entry.text == text && entry.rect.left() >= zone.left())
            .min_by(|left, right| left.rect.top().total_cmp(&right.rect.top()))
            .unwrap_or_else(|| panic!("the banner {zone:?} draws {text:?}: {drawn:?}"))
    };
    let head = find("Review work before continuing (1)");
    let (name, sentence, control) = (find(NAME), find(SENTENCE), find("Resume work"));
    assert!(
        (control.rect.center().y - name.rect.center().y).abs() < 4.0
            && (sentence.rect.center().y - name.rect.center().y).abs() < 4.0,
        "the name, the sentence and the control stand on one row: {name:?} {sentence:?} {control:?}"
    );
    assert!(
        sentence.cut && name.rect.right() < sentence.rect.left() && sentence.rect.right() < control.rect.left(),
        "the sentence is cut between the name and the control: {name:?} {sentence:?} {control:?}"
    );
    assert!(
        zone.height() <= 64.0 && control.rect.bottom() <= zone.bottom() && head.rect.top() >= zone.top(),
        "the banner of one session takes the height of its head and one row: {zone:?}"
    );

    // The whole sentence is the hover text of the cut one.
    let _ = window.frame(vec![Event::PointerMoved(sentence.rect.center())]);
    window.time += 2.0;
    let _ = window.frame(Vec::new());
    let hovered = texts(&window.frame(Vec::new()));
    assert!(
        hovered.iter().any(|entry| entry.text == SENTENCE && !entry.cut),
        "the hover text of the sentence shows it whole: {hovered:?}"
    );
}

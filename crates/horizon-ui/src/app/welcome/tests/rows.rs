//! The row of the section **Pick up where you left off**, at three widths.
//!
//! A row is one line of title and one line of detail. The detail names the
//! program at its start, the folder and the time, and the row keeps the program
//! and the end of the line when the folder is too long to show whole.

use std::path::{Path, PathBuf};

use egui::Context;
use horizon_core::{PanelKind, PickupFolder, PickupList, PickupRow, PickupState};

use super::{AppWindow, frame_shapes_at, painted_text_starting_with};
use crate::app::session_pickup::SessionPickup;
use crate::app::test_support::test_app;

/// The height of one row of the section.
const ROW_HEIGHT: f32 = 44.0;
/// The corner radius of the plate of one row.
const ROW_RADIUS: u8 = 10;
/// The space on both sides inside a row.
const ROW_PAD: f32 = 12.0;
/// The width that the press of a row keeps for itself.
const PRESS_WIDTH: f32 = 74.0;
/// The space between the text of a row and its press.
const TEXT_PRESS_GAP: f32 = 8.0;

/// A folder path that no row of the page holds at a narrow width.
const LONG_PATH: &str = "/a/long/path/that/keeps/going/and/going/more/and/more/until/it/ends/and/then/still/more/of/it/because/a/folder/can/be/very/deep/in/a/tree";

/// The detail of the row of a page that holds one session.
fn page_detail(size: [f32; 2], state: PickupState) -> (String, egui::Rect, egui::Rect) {
    let (_temp, mut app) = test_app();
    app.session_pickup = SessionPickup::with_list(PickupList::new(vec![PickupFolder {
        path: PathBuf::from(LONG_PATH),
        name: "shop".to_string(),
        parent: LONG_PATH.to_string(),
        exists: true,
        rows: vec![PickupRow {
            kind: PanelKind::Claude,
            session_id: "00000000-0000-0000-0000-000000000001".to_string(),
            title: "Fix the build".to_string(),
            last_active: "2 hours ago".to_string(),
            state,
        }],
    }]));
    let ctx = Context::default();
    let shapes = frame_shapes_at(&ctx, &mut app, size);
    let (shape, text) = painted_text_starting_with(&shapes, "Claude").expect("the detail of the row is drawn");
    assert!(
        !shape.galley.elided,
        "the detail of a row is cut in its own words, not at its end by the line: {size:?}"
    );
    let row = row_rect(&shapes);
    (shape.galley.text().to_owned(), text, row)
}

/// The plate of the row of the section, as the page painted it.
fn row_rect(shapes: &[egui::epaint::ClippedShape]) -> egui::Rect {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Rect(rect)
                if rect.corner_radius == egui::CornerRadius::same(ROW_RADIUS)
                    && (rect.rect.height() - ROW_HEIGHT).abs() < 0.6 =>
            {
                if !out.contains(&rect.rect) {
                    out.push(rect.rect);
                }
            }
            egui::Shape::Vec(inner) => {
                for shape in inner {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut out);
    }
    assert_eq!(out.len(), 1, "the page paints the plate of one row: {out:?}");
    out[0]
}

#[test]
fn the_detail_of_a_row_keeps_its_program_and_its_time_at_the_widths_of_a_window() {
    for (size, cut) in [
        ([1600.0, 1000.0], false),
        ([1000.0, 700.0], true),
        ([800.0, 600.0], true),
    ] {
        let (detail, text, row) = page_detail(size, PickupState::Ready);

        assert!(
            detail.starts_with("Claude · shop in "),
            "the detail starts with the program and the folder at {size:?}: {detail:?}"
        );
        assert!(
            detail.ends_with("2 hours ago"),
            "the time stands at the end of the detail at {size:?}: {detail:?}"
        );
        let room = row.max.x - ROW_PAD - PRESS_WIDTH - TEXT_PRESS_GAP;
        assert!(
            text.max.x <= room + 0.6,
            "the detail keeps the room of the press of the row at {size:?}: {} against {room}",
            text.max.x
        );
        if cut {
            assert!(
                detail.contains('…'),
                "a detail that does not fit is cut at {size:?}: {detail:?}"
            );
            assert!(
                detail.contains("/a/long/path/"),
                "the start of the path stays in the detail at {size:?}: {detail:?}"
            );
            assert!(
                detail.contains("deep/in/a/tree"),
                "the end of the path stays in the detail at {size:?}: {detail:?}"
            );
        } else {
            assert!(
                !detail.contains('…'),
                "a row that holds the whole detail cuts nothing at {size:?}: {detail:?}"
            );
            assert!(
                detail.contains(LONG_PATH),
                "the whole path is shown at {size:?}: {detail:?}"
            );
        }
    }
}

#[test]
fn the_state_of_a_row_stays_at_the_end_of_its_detail() {
    let (detail, _, _) = page_detail([800.0, 600.0], PickupState::FolderGone);

    assert!(
        detail.ends_with("folder is gone"),
        "the state of the row stands at the end of the detail: {detail:?}"
    );
    assert!(
        detail.contains("2 hours ago · folder is gone"),
        "the time stands before the state: {detail:?}"
    );
}

#[test]
fn the_row_of_a_wide_canvas_is_as_wide_as_the_block_of_the_page() {
    let size = [5000.0, 1348.0];
    let (_temp, mut app) = test_app();
    app.session_pickup = SessionPickup::with_list(PickupList::new(vec![PickupFolder {
        path: PathBuf::from(LONG_PATH),
        name: "shop".to_string(),
        parent: LONG_PATH.to_string(),
        exists: true,
        rows: vec![PickupRow {
            kind: PanelKind::Claude,
            session_id: "00000000-0000-0000-0000-000000000001".to_string(),
            title: "Fix the build".to_string(),
            last_active: "2 hours ago".to_string(),
            state: PickupState::Ready,
        }],
    }]));
    let ctx = Context::default();
    let shapes = frame_shapes_at(&ctx, &mut app, size);

    let row = row_rect(&shapes);
    let block = super::block_rect(&shapes);
    assert!(
        (row.width() - block.width()).abs() < 0.6,
        "the row is as wide as the block of the page: {} against {}",
        row.width(),
        block.width()
    );
    let (shape, _) = painted_text_starting_with(&shapes, "Claude").expect("the detail of the row is drawn");
    assert!(!shape.galley.elided, "a wide row is not cut by its line");
    assert!(
        !shape.galley.text().contains('…'),
        "a wide row holds the whole detail: {:?}",
        shape.galley.text()
    );
}

/// The session id of the row of the store of a test.
const SESSION: &str = "00000000-0000-0000-0000-000000000001";

/// One sentence of a page, as an agent keeps it as the last prompt of a session:
/// longer than the row of a narrow window holds and shorter than the row of a wide
/// one.
const PROMPT: &str = "Write the page about the first start and keep every sentence short enough for the \
                      row that shows it, then read the page again";

/// One Claude Code transcript, as that program writes it, under `home`.
fn claude_session(home: &Path, folder: &Path, prompt: &str) {
    let project = home.join(".claude/projects/-repo");
    std::fs::create_dir_all(&project).expect("the Claude store");
    let record = serde_json::json!({
        "type": "user",
        "sessionId": SESSION,
        "cwd": folder.display().to_string(),
    });
    let last_prompt = serde_json::json!({"type": "last-prompt", "lastPrompt": prompt});
    std::fs::write(
        project.join(format!("{SESSION}.jsonl")),
        format!("{record}\n{last_prompt}\n"),
    )
    .expect("the transcript");
}

/// The title of the row of the page of `size`, from the frames of the whole
/// application: the text that the row shows, whether the layout cut it, and the
/// room that the title has between the pads of the row and its press.
fn page_title(size: [f32; 2], prompt: &str) -> (String, bool, egui::Rect, f32) {
    let store = tempfile::tempdir().expect("temp dir");
    let home = store.path().join("home");
    let folder = home.join("repo");
    std::fs::create_dir_all(&folder).expect("the folder of the session");
    claude_session(&home, &folder, prompt);

    let mut window = AppWindow::with_state(size, |app| {
        app.session_pickup = SessionPickup::reading(home.clone());
    });
    window.frames_until("the pickup read the store of the test", |app| {
        !app.session_pickup.first_page().is_empty()
    });
    let shapes = window.frame(Vec::new());
    let starts = prompt.chars().take(10).collect::<String>();
    let (shape, text) = painted_text_starting_with(&shapes, &starts).expect("the title of the row is drawn");
    let row = row_rect(&shapes);
    let room = row.width() - 2.0 * ROW_PAD - PRESS_WIDTH - TEXT_PRESS_GAP;
    assert!(
        text.width() <= room + 0.6,
        "the title keeps the room of the press of the row at {size:?}: {} against {room}",
        text.width()
    );
    (shape.galley.text().to_owned(), shape.galley.elided, text, room)
}

/// A wide row shows the whole stored title, and a narrow row cuts it at the end of
/// the room that it has: the stored bound is not what the person sees.
#[test]
fn the_title_of_a_row_uses_the_width_of_the_row() {
    let (wide_stored, wide_elided, wide_text, _) = page_title([5000.0, 1348.0], PROMPT);
    assert_eq!(
        wide_stored,
        PROMPT,
        "the title of the row of a wide window is the stored title, with {} characters",
        PROMPT.chars().count()
    );
    assert!(
        !wide_elided,
        "the row of a block of 1480 points shows the whole stored title: {} points of text",
        wide_text.width()
    );

    let (narrow_stored, narrow_elided, narrow_text, room) = page_title([800.0, 600.0], PROMPT);
    assert_eq!(
        narrow_stored, PROMPT,
        "the stored title does not change with the width of the window"
    );
    assert!(
        narrow_elided,
        "the row of a narrow window cuts the title at its end: {} points of text",
        narrow_text.width()
    );
    // The cut stands between two words, so the text ends at most one word
    // before the end of its room.
    assert!(
        narrow_text.width() >= room * 0.9,
        "the row uses the room that it has: {} of {room} points",
        narrow_text.width()
    );
}

/// A title of exactly the number of characters that a store keeps: the longest
/// title that a row can ever show.
fn title_at_the_bound() -> String {
    let sentence = "the row of the page shows every word of this title on its own line ";
    let mut title: String = sentence
        .chars()
        .cycle()
        .take(horizon_core::session_pickup::MAX_TITLE_CHARS)
        .collect();
    // The store trims the white space at the end of a title.
    if title.ends_with(' ') {
        title.pop();
        title.push('.');
    }
    title
}

/// The widest block of the page holds a title of the bound of the store whole, so
/// the bound is what a row can show and not a count that cuts a stored title.
#[test]
fn the_widest_row_holds_a_title_of_the_bound_whole() {
    let title = title_at_the_bound();
    assert_eq!(
        title.chars().count(),
        horizon_core::session_pickup::MAX_TITLE_CHARS,
        "the title of this test is exactly at the bound"
    );
    let (stored, elided, text, _) = page_title([5000.0, 1348.0], &title);
    assert_eq!(stored, title, "the store keeps a title at the bound whole");
    assert!(
        !elided,
        "the row of the widest block holds a title at the bound whole: {} points of text",
        text.width()
    );
}

//! The row of the section **Pick up where you left off**, at three widths.
//!
//! A row is one line of title and one line of detail. The detail names the
//! program at its start, the folder and the time, and the row keeps the program
//! and the end of the line when the folder is too long to show whole.

use std::path::PathBuf;

use egui::Context;
use horizon_core::{PanelKind, PickupFolder, PickupList, PickupRow, PickupState};

use super::{frame_shapes_at, painted_text_starting_with};
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
                detail.contains("/a/long/path/that"),
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

//! The name and the word of a state of a panel row: the two texts share one line,
//! so the row gives each one a room and never lets them cover each other.

use super::attention_ledger;
use crate::test_egui::DiscardTextures;

#[test]
fn the_word_of_a_panel_state_shares_a_row_that_is_too_narrow_for_it() {
    use super::super::panel_rows::{STATE_GAP, title_and_state};
    // A row of a sidebar 168 wide, which is narrower than the sidebar of the
    // window. Its rows start 8 in, the card ends 8 before the edge, the title
    // starts at 68 and the row keeps 26 for a close.
    let right = 160.0 - 26.0;
    let title_left = 68.0;
    let room = right - title_left;
    // The words of the states at the font of a row: background, needs you,
    // working and done.
    for state_width in [54.0, 48.2, 40.5, 22.8] {
        let (title_right, state_left, width) =
            title_and_state(right, title_left, state_width).expect("a row with a state word");
        assert!(
            (state_left + width - right).abs() <= f32::EPSILON,
            "the word ends at the right edge of the row's text"
        );
        assert!(state_left + width <= 168.0, "the word stays inside the sidebar");
        assert!(width <= state_width, "a word that does not fit is cut, and never grows");
        assert!(
            width <= room / 2.0 + f32::EPSILON,
            "the word never takes more than half of the row: {width}"
        );
        assert!(title_right >= title_left, "the title keeps a room to elide in");
        assert!(
            (title_right + STATE_GAP - state_left).abs() <= f32::EPSILON,
            "the two texts keep the gap between them, so the one never covers the other"
        );
    }
    // Without a state word the title runs to the right edge.
    assert_eq!(title_and_state(right, title_left, 0.0), None);
}

#[test]
fn the_default_sidebar_shows_the_whole_word_of_every_state() {
    use super::super::panel_rows::{STATE_GAP, title_and_state};
    // The sidebar is 210 wide, with the same insets as the narrow row.
    let right = 210.0 - 8.0 - 26.0;
    let title_left = 68.0;
    for state_width in [54.0, 48.2, 40.5, 22.8] {
        let (title_right, state_left, width) =
            title_and_state(right, title_left, state_width).expect("a row with a state word");
        assert!(
            (width - state_width).abs() <= f32::EPSILON,
            "the whole word of a state fits the default row"
        );
        assert!(
            title_right - title_left > 0.0,
            "the name of the panel keeps a room of its own"
        );
        assert_eq!(title_right + STATE_GAP, state_left);
        assert!((state_left + width - right).abs() <= f32::EPSILON);
    }
}

#[test]
fn the_name_and_the_state_of_a_panel_row_never_cover_each_other() {
    let (_temp, ctx, mut app) =
        crate::app::test_support::test_app_with_startup(horizon_core::StartupDecision::Ephemeral {
            runtime_state: Box::new(horizon_core::RuntimeState::default()),
        });
    let workspace = app.board.create_workspace("Lane");
    let panel = app
        .board
        .create_panel(
            horizon_core::PanelOptions {
                name: Some("Zeta sync lane".to_owned()),
                kind: horizon_core::PanelKind::Claude,
                command: Some("seed".to_owned()),
                ..horizon_core::PanelOptions::default()
            },
            workspace,
        )
        .expect("panel");
    app.board.focused = None;
    let now = horizon_core::current_unix_millis();
    let local = app.board.panel(panel).expect("panel").local_id.clone();
    for (index, (state, background_tasks, word)) in [
        (horizon_core::agent_work::TurnState::Working, 0, "Working"),
        (horizon_core::agent_work::TurnState::Finished, 1, "Background"),
        (horizon_core::agent_work::TurnState::Finished, 0, "Done"),
        (horizon_core::agent_work::TurnState::Blocked, 0, "Needs you"),
    ]
    .into_iter()
    .enumerate()
    {
        // The panel saw a turn before the one that ends here, so its end is unseen.
        let at = now - 4 + i64::try_from(index).expect("index");
        let mut ledger = attention_ledger(state, at);
        ledger.background_tasks = background_tasks;
        app.apply_attention_values(vec![(panel, local.clone(), Some(ledger))], &ctx);
        assert_eq!(app.panel_attention(&local).word(), Some(word), "the state of the panel");
        // The sidebar keeps one width, also in a window that is narrower than
        // the smallest window that Horizon opens.
        for viewport in [1600.0_f32, 360.0] {
            let sidebar = crate::app::SIDEBAR_WIDTH;
            let rects = sidebar_text_rects(&ctx, &mut app, viewport);
            let name = rects
                .iter()
                .find(|drawn| drawn.text.starts_with("Zeta") && drawn.rect.left() < sidebar / 2.0)
                .unwrap_or_else(|| panic!("the drawn name of the panel: {rects:?}"));
            let prefix = &word[..2];
            // The word of the state shares the line of the name and stands to its
            // right; the toolbar of the sidebar has words of its own.
            let row = name.rect.top()..=name.rect.bottom();
            let state = rects
                .iter()
                .find(|drawn| {
                    drawn.text.starts_with(prefix)
                        && drawn.rect.left() > name.rect.left()
                        && row.contains(&drawn.rect.center().y)
                })
                .unwrap_or_else(|| panic!("the drawn word {word:?} of the state: {rects:?}"));
            assert!(
                name.rect.right() + 0.5 <= state.rect.left(),
                "the name {:?} ends before the word {word:?} begins {:?}",
                name.text,
                state.text()
            );
            assert!(
                !name.rect.intersects(state.rect),
                "the two texts never cover each other: {name:?} {state:?}"
            );
            assert!(
                state.rect.left() >= sidebar / 2.0,
                "the word of the state stays in the right half of the row"
            );
            assert!(!state.cut, "the whole word of the state shows: {:?}", state.text);
            assert!(
                name.drawn >= 5,
                "the name keeps a read of it beside the whole word: {:?}",
                name.text
            );
        }
    }
}

/// One text of a frame of the sidebar, with the rectangle it takes.
#[derive(Clone, Debug)]
struct DrawnText {
    text: String,
    rect: egui::Rect,
    /// The characters that the row shows, without the mark of a cut.
    drawn: usize,
    /// Whether the row cut the text.
    cut: bool,
}

impl DrawnText {
    fn text(&self) -> String {
        self.text.clone()
    }
}

/// The drawn texts of one frame of the sidebar of `viewport` wide.
fn sidebar_text_rects(ctx: &egui::Context, app: &mut crate::app::HorizonApp, viewport: f32) -> Vec<DrawnText> {
    let mut input = egui::RawInput::default();
    input.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            native_pixels_per_point: Some(1.0),
            inner_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(viewport, 720.0))),
            ..egui::ViewportInfo::default()
        },
    );
    let mut rects = Vec::new();
    // A panel row shows from its second frame on.
    for _ in 0..2 {
        let output = ctx
            .run_ui(input.clone(), |ui| app.render_sidebar(ui.ctx()))
            .discard_textures();
        rects.clear();
        for clipped in &output.shapes {
            collect_text_rects(&clipped.shape, &mut rects);
        }
    }
    rects
}

fn collect_text_rects(shape: &egui::Shape, rects: &mut Vec<DrawnText>) {
    match shape {
        egui::Shape::Text(text) => {
            let glyphs = text.galley.rows.first().map_or(0, |row| row.glyphs.len());
            rects.push(DrawnText {
                text: text.galley.text().to_owned(),
                rect: egui::Rect::from_min_size(text.pos, text.galley.size()),
                drawn: if text.galley.elided {
                    glyphs.saturating_sub(1)
                } else {
                    glyphs
                },
                cut: text.galley.elided,
            });
        }
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_text_rects(shape, rects);
            }
        }
        _ => {}
    }
}

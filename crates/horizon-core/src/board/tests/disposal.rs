use super::super::*;
use super::editor_panel_options;

#[test]
fn a_panel_hidden_for_disposal_loses_its_focus_and_returns_only_if_asked() {
    let mut board = Board::new();
    let workspace = board.create_workspace("cloud");
    let panel = board.create_panel(editor_panel_options(), workspace).unwrap();
    let other = board.create_panel(editor_panel_options(), workspace).unwrap();
    board.focused = Some(panel);

    assert!(board.hide_for_disposal(panel));
    assert!(!board.panel(panel).unwrap().visible);
    assert!(board.is_hidden_for_disposal(panel));
    assert_eq!(board.focused, None, "focus never stays on a panel that cannot be seen");
    assert!(!board.hide_for_disposal(panel), "it was no longer showing");
    assert!(!board.is_hidden_for_disposal(other), "other panels are untouched");

    board.end_disposal_hiding(panel, true);
    assert!(board.panel(panel).unwrap().visible);
    assert!(!board.is_hidden_for_disposal(panel));

    assert!(board.hide_for_disposal(panel));
    board.end_disposal_hiding(panel, false);
    assert!(
        !board.panel(panel).unwrap().visible,
        "a collapsed cloud shows it on expand instead"
    );
    assert!(!board.is_hidden_for_disposal(panel));
}

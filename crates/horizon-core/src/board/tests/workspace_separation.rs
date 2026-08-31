use crate::layout::WS_COLLISION_GAP;

use super::super::arrangement::rects_overlap;
use super::super::*;
use super::editor_panel_options;

#[test]
fn separating_overlapping_workspace_preserves_existing_freeform_layout() {
    let mut board = Board::new();
    let left = board.create_workspace_at("left", [0.0, 40.0]);
    let reattached = board.create_workspace_at("reattached", [500.0, 40.0]);
    let right = board.create_workspace_at("right", [900.0, 280.0]);

    for workspace_id in [left, reattached, right] {
        board
            .create_panel(editor_panel_options(), workspace_id)
            .expect("panel should spawn");
    }

    let left_frame = board.workspace_frame_rect(left).expect("left frame");
    let reattached_frame = board.workspace_frame_rect(reattached).expect("reattached frame");
    assert!(board.translate_workspace(
        reattached,
        [
            left_frame[0] + 5.0 - reattached_frame[0],
            left_frame[1] + 5.0 - reattached_frame[1],
        ],
    ));
    assert!(rects_overlap(
        board.workspace_frame_rect(reattached).expect("overlapping frame"),
        left_frame,
    ));
    let right_frame = board.workspace_frame_rect(right).expect("right frame");
    assert!(board.translate_workspace(
        right,
        [
            left_frame[0] + 10.0 - right_frame[0],
            left_frame[1] + 10.0 - right_frame[1],
        ],
    ));
    let left_before = board.workspace(left).expect("left workspace").position;
    let right_before = board.workspace(right).expect("right workspace").position;
    assert!(rects_overlap(
        left_frame,
        board.workspace_frame_rect(right).expect("overlapping existing frame"),
    ));

    assert!(board.separate_workspace_from_overlaps_in_scope(reattached, &[left, reattached, right]));

    let reattached_frame = board.workspace_frame_rect(reattached).expect("reattached frame");
    let right_frame = board.workspace_frame_rect(right).expect("right frame");
    assert_eq!(
        board
            .workspace(left)
            .expect("left workspace")
            .position
            .map(f32::to_bits),
        left_before.map(f32::to_bits)
    );
    assert_eq!(
        board
            .workspace(right)
            .expect("right workspace")
            .position
            .map(f32::to_bits),
        right_before.map(f32::to_bits)
    );
    assert!(!rects_overlap(reattached_frame, left_frame));
    assert!(!rects_overlap(reattached_frame, right_frame));
    assert!((reattached_frame[0] - (right_frame[2] + WS_COLLISION_GAP)).abs() <= f32::EPSILON);
    assert!(!board.separate_workspace_from_overlaps_in_scope(reattached, &[left, reattached, right]));
}

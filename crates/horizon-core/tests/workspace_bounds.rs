use horizon_core::{Board, PanelOptions};

fn vec2_eq(left: [f32; 2], right: [f32; 2]) -> bool {
    (left[0] - right[0]).abs() <= f32::EPSILON && (left[1] - right[1]).abs() <= f32::EPSILON
}

#[test]
fn workspace_bounds_map_matches_individual_workspace_bounds() {
    let mut board = Board::new();
    let alpha = board.create_workspace("alpha");
    let beta = board.create_workspace("beta");
    let gamma = board.create_workspace("gamma");

    board
        .create_panel(
            PanelOptions {
                position: Some([100.0, 120.0]),
                size: Some([300.0, 220.0]),
                ..PanelOptions::default()
            },
            alpha,
        )
        .expect("alpha panel should spawn");
    board
        .create_panel(
            PanelOptions {
                position: Some([640.0, 140.0]),
                size: Some([260.0, 180.0]),
                ..PanelOptions::default()
            },
            alpha,
        )
        .expect("second alpha panel should spawn");
    board
        .create_panel(
            PanelOptions {
                position: Some([1500.0, 360.0]),
                size: Some([420.0, 260.0]),
                ..PanelOptions::default()
            },
            beta,
        )
        .expect("beta panel should spawn");

    let bounds = board.workspace_bounds_map();

    let alpha_bounds = bounds.get(&alpha).copied().expect("alpha should have bounds");
    let beta_bounds = bounds.get(&beta).copied().expect("beta should have bounds");

    let expected_alpha = board.workspace_bounds(alpha).expect("alpha bounds");
    let expected_beta = board.workspace_bounds(beta).expect("beta bounds");

    assert!(vec2_eq(alpha_bounds.0, expected_alpha.0));
    assert!(vec2_eq(alpha_bounds.1, expected_alpha.1));
    assert!(vec2_eq(beta_bounds.0, expected_beta.0));
    assert!(vec2_eq(beta_bounds.1, expected_beta.1));
    assert!(!bounds.contains_key(&gamma));
}

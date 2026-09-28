//! Per-frame card state: output follow and the drawer of a collapsed cloud.
use super::*;

#[test]
fn a_view_no_longer_shown_stops_holding_new_output_aside() {
    let mut runtime = super::super::super::Runtime {
        verbose_unpinned: true,
        ..Default::default()
    };
    // A frame in which no output view reported being scrolled up.
    output::begin_frame(&mut runtime);
    assert!(!runtime.verbose_unpinned);
    runtime.push_log("fresh line".into());
    assert_eq!(runtime.logs.back().map(|line| line.text.as_str()), Some("fresh line"));
    assert!(runtime.pending_logs.is_empty());
    runtime.unpinned_views = 2;
    output::begin_frame(&mut runtime);
    assert!(runtime.verbose_unpinned, "a view still scrolled up keeps holding lines");
}

#[test]
#[cfg(unix)]
fn opening_the_drawer_of_a_collapsed_cloud_expands_it() {
    let (temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("cloud fixture");
    let mut group = horizon_core::cloud_panel::CloudGroup::new(
        7,
        "Collapsed".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        temp.path().into(),
        [0.0, 0.0],
    );
    group.remote = Some(size_launch());
    group.collapsed = true;
    app.cloud_prototype.groups.0.push(group);
    app.cloud_prototype.production.runtimes.entry(7).or_default();
    let ctx = egui::Context::default();
    app.apply_strip_action(7, strip::StripAction::Primary(status::Primary::Stop), &ctx);
    assert!(!app.cloud_prototype.groups.0[0].collapsed);
    let runtime = &app.cloud_prototype.production.runtimes[&7];
    assert_eq!(runtime.drawer, Some(Tab::Manage));
    assert!(runtime.confirmation == Confirmation::Stop);
}

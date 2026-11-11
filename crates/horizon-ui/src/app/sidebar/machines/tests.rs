//! The rows of the machine section, read only when they change.

use super::super::super::test_support::{run_app_frame, test_app};
use super::super::SidebarActions;
use super::{MachineRow, MachineRows, MachineState, machine_click, spoken};
use horizon_core::{Machine, MachineRegistry, Machines, PanelId, SshConnectionStatus};

fn record(name: &str, provider: Option<&str>) -> Machine {
    Machine {
        name: name.to_owned(),
        host: "203.0.113.10".to_owned(),
        user: Some("root".to_owned()),
        provider: provider.map(str::to_owned),
        added_at_millis: 1,
        ..Machine::default()
    }
}

fn with_machines(app: &mut crate::app::HorizonApp, records: Vec<Machine>) {
    app.machine_list = Machines::with_records(MachineRegistry::from_home(app.session_store.home()), records);
}

#[test]
fn no_machine_leaves_the_section_out() {
    let (_temp, mut app) = test_app();
    app.refresh_machine_rows();
    assert!(app.machine_rows.rows().is_empty(), "an empty list draws no section");
}

#[test]
fn one_machine_of_the_list_is_one_row_in_name_order() {
    let (_temp, mut app) = test_app();
    with_machines(
        &mut app,
        vec![record("build-2", None), record("build-1", Some("Hetzner, hel1"))],
    );
    app.refresh_machine_rows();
    let rows = app.machine_rows.rows();
    assert_eq!(
        rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
        ["build-1", "build-2"],
        "the rows follow the name order of the list"
    );
    assert_eq!(rows[0].provider.as_deref(), Some("Hetzner, hel1"));
    assert_eq!(
        rows[0].state,
        MachineState::Closed,
        "no terminal of the machine is open"
    );
    assert_eq!(rows[0].panel, None);
}

#[test]
fn the_rows_are_read_again_only_when_the_list_changes() {
    let (_temp, mut app) = test_app();
    with_machines(&mut app, vec![record("build-1", None)]);
    app.refresh_machine_rows();
    let first = app.machine_rows.read_from();
    app.refresh_machine_rows();
    assert_eq!(
        first,
        app.machine_rows.read_from(),
        "an unchanged list keeps the rows it read"
    );

    with_machines(&mut app, vec![record("build-1", Some("Hetzner, hel1"))]);
    app.refresh_machine_rows();
    assert_eq!(
        app.machine_rows.rows()[0].provider.as_deref(),
        Some("Hetzner, hel1"),
        "a changed record is read again"
    );
}

#[test]
fn a_row_says_the_state_of_its_own_terminal() {
    for (status, state) in [
        (None, MachineState::Closed),
        (Some(SshConnectionStatus::Connecting), MachineState::Connecting),
        (Some(SshConnectionStatus::Reconnecting), MachineState::Reconnecting),
        (Some(SshConnectionStatus::Connected), MachineState::Connected),
        (Some(SshConnectionStatus::Disconnected), MachineState::Disconnected),
    ] {
        assert_eq!(MachineState::of(status), state);
        assert!(
            !state.spoken().is_empty(),
            "each state tells a screen reader what it is"
        );
    }
}

#[test]
fn a_machine_that_waits_for_the_server_says_so() {
    let row = MachineRow {
        name: "build-1".into(),
        provider: Some("Hetzner, hel1".into()),
        state: MachineState::Closed,
        pending: true,
        panel: None,
        workspace: None,
    };
    let line = spoken(&row);
    assert!(line.contains("build-1"), "{line}");
    assert!(line.contains("no terminal open"), "{line}");
    assert!(line.contains("pending"), "{line}");
    let quiet = MachineRow { pending: false, ..row };
    assert!(!spoken(&quiet).contains("pending"), "a settled row says nothing");
}

#[test]
fn a_press_on_a_closed_machine_opens_one_and_a_press_on_an_open_machine_focuses_it() {
    let mut closed = MachineRow {
        name: "build-1".into(),
        provider: None,
        state: MachineState::Closed,
        pending: false,
        panel: None,
        workspace: None,
    };
    let mut actions = SidebarActions::default();
    machine_click(&closed, &mut actions);
    assert_eq!(actions.connect_machine.as_deref(), Some("build-1"));
    assert_eq!(actions.focus_panel, None, "a closed machine opens a terminal first");

    closed.state = MachineState::Connected;
    closed.panel = Some(PanelId(7));
    closed.workspace = Some(horizon_core::WorkspaceId(3));
    let mut actions = SidebarActions::default();
    machine_click(&closed, &mut actions);
    assert_eq!(
        actions.connect_machine, None,
        "an open machine opens no second terminal"
    );
    assert_eq!(actions.focus_panel, Some(PanelId(7)));
    assert_eq!(
        actions.pan_to_panel,
        Some(PanelId(7)),
        "the click switches to its workspace"
    );
}

#[test]
fn the_section_draws_without_a_machine_and_with_one() {
    use super::super::super::test_support::test_app_with_startup;
    use horizon_core::{RuntimeState, StartupDecision};

    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.settle_browser_host_io();
    run_app_frame(&ctx, &mut app);
    // The startup read of the machine list answers before the test writes one,
    // or the empty list of the home replaces the record it wrote.
    crate::app::block_machines::settle_machine_load(&mut app);
    assert!(app.machine_rows.rows().is_empty(), "no machine draws no section");

    with_machines(&mut app, vec![record("build-1", Some("Hetzner, hel1"))]);
    app.refresh_machine_rows();
    run_app_frame(&ctx, &mut app);
    assert_eq!(app.machine_rows.rows().len(), 1, "one machine draws one row");
}

#[test]
fn the_section_takes_no_room_without_a_machine() {
    let rows: MachineRows = MachineRows::default();
    assert!(rows.rows().is_empty(), "an empty cache has no row to draw");
}

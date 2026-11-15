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

fn with_repository(name: &str, repository: &str) -> Machine {
    Machine {
        repository: Some(repository.to_owned()),
        ..record(name, Some("Hetzner, hel1"))
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
    assert_eq!(rows[0].second.as_deref(), Some("Hetzner, hel1"));
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
        app.machine_rows.rows()[0].second.as_deref(),
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
        second: Some("Hetzner, hel1".into()),
        state: MachineState::Closed,
        pending: true,
        panel: None,
        workspace: None,
        watch: super::MachineWatchRow::default(),
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
        second: None,
        state: MachineState::Closed,
        pending: false,
        panel: None,
        workspace: None,
        watch: super::MachineWatchRow::default(),
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
    assert!(
        rows.server_line.is_none(),
        "a list of one computer shows no line of a server"
    );
}

#[test]
fn the_line_of_the_server_draws_before_the_first_machine() {
    use super::super::super::test_support::{run_app_frame, test_app};

    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    crate::app::block_machines::settle_machine_load(&mut app);
    // The line of a link that this test does not run: the fingerprint says that
    // the rows were read from this list, so the frame draws them as they are.
    app.machine_rows = MachineRows {
        rows: Vec::new(),
        server_line: Some("Server: in step".to_owned()),
        server_reason: None,
        read_from: Some(super::machine_fingerprint(&app)),
    };

    run_app_frame(&ctx, &mut app);

    assert_eq!(
        app.machine_rows.server_line.as_deref(),
        Some("Server: in step"),
        "the section keeps the line of its server without a machine"
    );
    assert!(app.machine_rows.rows().is_empty());
}

#[test]
fn the_second_line_of_a_row_names_the_repository_beside_the_provider_words() {
    let (_temp, mut app) = test_app();
    with_machines(
        &mut app,
        vec![
            with_repository("build-1", "example-org/shop"),
            record("build-2", Some("Hetzner, hel1")),
            record("build-3", None),
        ],
    );
    app.refresh_machine_rows();
    let rows = app.machine_rows.rows();
    assert_eq!(
        rows[0].second.as_deref(),
        Some("example-org/shop \u{00b7} Hetzner, hel1"),
        "the repository stands beside the provider words"
    );
    assert_eq!(
        rows[1].second.as_deref(),
        Some("Hetzner, hel1"),
        "a record without one keeps its line"
    );
    assert_eq!(rows[2].second, None, "a record that names neither draws no second line");

    // A changed repository is read again, because the row is read from the list.
    with_machines(&mut app, vec![with_repository("build-1", "example-org/docs")]);
    app.refresh_machine_rows();
    assert_eq!(
        app.machine_rows.rows()[0].second.as_deref(),
        Some("example-org/docs \u{00b7} Hetzner, hel1"),
        "the row follows the repository of its record"
    );
}

/// The window that the frames of these tests paint.
const WINDOW: [f32; 2] = [1480.0, 1200.0];

/// The words that `ssh` prints when the server refuses the forward of a card.
const REFUSED_FORWARD: &str = "remote port forwarding failed for listen path /tmp/horizon-cards/build-1/night.sock";

/// How long a test of the card link waits for the work of the supervisor.
const CARD_LINK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

/// Keeps `build-1` in the list of the app, with the mark of its card setup.
fn with_cards(app: &mut crate::app::HorizonApp) {
    with_machines(
        app,
        vec![Machine {
            name: "build-1".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: Some(2222),
            cards: true,
            ..Machine::default()
        }],
    );
}

/// The machine that the card link of the app serves: one session of `build-1`.
fn card_machine(app: &crate::app::HorizonApp) -> crate::app::node_cards::CardMachine {
    crate::app::node_cards::CardMachine {
        name: "build-1".to_owned(),
        login: horizon_core::machine_watch::connection(
            &app.machine_list,
            &horizon_core::MachineKeys::from_home(app.session_store.home()),
            "build-1",
        )
        .expect("the login of the machine"),
        sessions: vec!["night".to_owned()],
        links: crate::app::node_cards::Links::clone(&app.node_cards.links),
    }
}

/// The one text of this pass that starts with `start`, if the pass drew it.
fn drawn_starting_with(output: &egui::FullOutput, start: &str) -> Option<String> {
    output.shapes.iter().find_map(|clipped| match &clipped.shape {
        egui::Shape::Text(shape) if shape.galley.text().starts_with(start) => Some(shape.galley.text().to_owned()),
        _ => None,
    })
}

/// Whether a screen reader hears `label` in this pass.
fn screen_reader_hears(output: &egui::FullOutput, label: &str) -> bool {
    output.platform_output.accesskit_update.as_ref().is_some_and(|update| {
        update
            .nodes
            .iter()
            .any(|(_, node)| node.label().or_else(|| node.value()).is_some_and(|text| text == label))
    })
}

/// Runs frames of `window` until `wanted` holds, and answers the last pass.
fn frame_until(
    ctx: &egui::Context,
    app: &mut crate::app::HorizonApp,
    window: [f32; 2],
    what: &str,
    wanted: impl Fn(&crate::app::HorizonApp) -> bool,
) -> egui::FullOutput {
    use crate::app::test_support::{raw_input, run_app_frame_with_input};

    let deadline = std::time::Instant::now() + CARD_LINK_DEADLINE;
    loop {
        let output = run_app_frame_with_input(ctx, app, raw_input(window, None));
        if wanted(app) {
            return output;
        }
        assert!(std::time::Instant::now() < deadline, "the test waited for {what}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn a_card_link_that_is_not_up_says_so_under_its_machine() {
    use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};
    use horizon_core::browser::manifest;
    use horizon_core::stand_in_program_in;
    use horizon_core::{RuntimeState, StartupDecision};

    for (words, down) in [
        (format!("printf '{REFUSED_FORWARD}\\n' >&2\nexit 255\n"), true),
        ("sleep 30\n".to_owned(), false),
    ] {
        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        ctx.enable_accesskit();
        // No watch of the agents of the machine: this test judges the card link, and
        // the quiet line of the watch would take the line of the card link away.
        app.template_config.machines.watch_agents = false;
        crate::app::block_machines::settle_machine_load(&mut app);
        let folder = tempfile::tempdir().expect("a folder of the stand-in");
        let ssh = stand_in_program_in(folder.path(), "ssh", &words).expect("the stand-in ssh");
        app.node_cards.program = ssh.to_string_lossy().into_owned();
        with_cards(&mut app);
        let _ = frame_until(&ctx, &mut app, WINDOW, "the machine of the watch", |app| {
            app.machine_watch
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.machine("build-1").is_some())
        });

        // The app serves one session of the machine, and the supervisor starts its child.
        app.node_cards
            .set_sessions(vec![card_machine(&app)], manifest::host_instance());
        let deadline = std::time::Instant::now() + CARD_LINK_DEADLINE;
        while down && app.node_cards.refusal("build-1").is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the test waited for the end of the card link"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !down {
            // A child that holds its link has no end to name.
            std::thread::sleep(std::time::Duration::from_millis(300));
            assert!(
                app.node_cards.refusal("build-1").is_none(),
                "no line of a link that is up"
            );
        }

        let mut output = run_app_frame_with_input(&ctx, &mut app, raw_input(WINDOW, None));
        for _ in 0..2 {
            output = run_app_frame_with_input(&ctx, &mut app, raw_input(WINDOW, None));
        }

        let line = drawn_starting_with(&output, super::agents::CARDS_DOWN);
        if down {
            assert_eq!(
                line.as_deref(),
                Some(super::agents::CARDS_DOWN),
                "the line of the card link"
            );
            let spoken = format!(
                "{} The last words of ssh were: {REFUSED_FORWARD}",
                horizon_core::machine_watch::WatchRefusal::Other.sentence()
            );
            assert!(
                screen_reader_hears(&output, &spoken),
                "a screen reader hears why the card link is not up, with the last words of ssh"
            );
        } else {
            assert!(line.is_none(), "a child that holds its link draws no line: {line:?}");
        }
    }
}

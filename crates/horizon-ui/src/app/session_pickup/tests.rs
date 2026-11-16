//! The pickup from the rows to the screen: the section of the first start page,
//! the window of the full list, and the open of one row.
//!
//! The stores of a test are made in the home of the test. No file of the home of a
//! person is read, and no agent program starts.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use egui::Context;
use horizon_core::{
    AgentSessionKey, PanelKind, PickupFolder, PickupList, PickupProgram, PickupRow, PickupState, PresetConfig,
    StartupDecision,
};

use super::rows::{OPEN, OPEN_ALL, OPEN_ANYWAY, PickupPress, folder_text, open_label, row_lines, spoken};
use super::{
    EMPTY_LINE, EVERY_ONE_OPEN_LINE, MENU_TITLE, SECTION_TITLE, SHOW_ALL, SessionPickup, WINDOW_TITLE, folder_name,
    summary_line, window_body_height,
};
use crate::app::root_chrome::ToolbarAction;
use crate::app::test_support::{raw_input, run_app_frame, test_app, test_app_with_startup};
use crate::test_egui::DiscardTextures;

const SESSION: &str = "00000000-0000-0000-0000-000000000001";
const OTHER_SESSION: &str = "00000000-0000-0000-0000-000000000002";

fn ready_row() -> PickupRow {
    PickupRow {
        kind: PanelKind::Claude,
        session_id: SESSION.to_string(),
        title: "Fix the build".to_string(),
        last_active: "2 hours ago".to_string(),
        state: PickupState::Ready,
    }
}

fn folder(path: &Path, name: &str, rows: Vec<PickupRow>) -> PickupFolder {
    PickupFolder {
        path: path.to_path_buf(),
        name: name.to_string(),
        parent: "~/github".to_string(),
        exists: true,
        rows,
    }
}

/// Every text that the first start page painted alone.
fn page_texts(ctx: &Context, app: &mut crate::app::HorizonApp) -> Vec<String> {
    let mut texts = Vec::new();
    for _ in 0..2 {
        texts.clear();
        let output = ctx.run_ui(raw_input([1200.0, 900.0], None), |ui| {
            app.render_welcome_page(ui.ctx());
        });
        let output = output.discard_textures();
        for clipped in &output.shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        if !texts.is_empty() {
            break;
        }
    }
    texts
}

/// Every text that the window of the full list painted.
fn window_texts(ctx: &Context, app: &mut crate::app::HorizonApp) -> Vec<String> {
    let mut texts = Vec::new();
    for _ in 0..2 {
        texts.clear();
        let output = ctx.run_ui(raw_input([1200.0, 900.0], None), |ui| {
            app.render_recent_agent_sessions(ui.ctx());
        });
        let output = output.discard_textures();
        for clipped in &output.shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        if !texts.is_empty() {
            break;
        }
    }
    texts
}

fn collect_texts(shape: &egui::Shape, texts: &mut Vec<String>) {
    match shape {
        egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_texts(shape, texts);
            }
        }
        _ => {}
    }
}

/// An app of the first start page whose pickup already holds `folders`.
fn app_with_pickup(folders: Vec<PickupFolder>) -> (tempfile::TempDir, Context, crate::app::HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(horizon_core::RuntimeState::default()),
    });
    app.session_pickup = SessionPickup::with_list(PickupList::new(folders));
    (temp, ctx, app)
}

#[test]
fn the_section_shows_the_newest_row_of_each_folder_with_its_own_press() {
    let (_temp, ctx, mut app) = app_with_pickup(vec![
        folder(Path::new("/tmp/one"), "one", vec![ready_row()]),
        folder(
            Path::new("/tmp/two"),
            "two",
            vec![PickupRow {
                session_id: OTHER_SESSION.to_string(),
                state: PickupState::FolderGone,
                ..ready_row()
            }],
        ),
    ]);

    let texts = page_texts(&ctx, &mut app);

    assert!(texts.iter().any(|text| text == SECTION_TITLE), "{texts:?}");
    assert!(texts.iter().any(|text| text == SHOW_ALL), "{texts:?}");
    assert!(texts.iter().any(|text| text == "Fix the build"), "{texts:?}");
    assert!(
        texts.iter().any(|text| text.contains("one in ~/github")),
        "the row names its folder: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text.contains("folder is gone")),
        "the row says that its folder is gone: {texts:?}"
    );
}

#[test]
fn the_first_start_page_without_a_session_is_the_page_of_today() {
    let (_temp, ctx, mut app) = app_with_pickup(Vec::new());

    let texts = page_texts(&ctx, &mut app);
    let (block, section) = app.welcome_block_heights_for_test(&ctx);

    assert!(
        !texts.iter().any(|text| text == SECTION_TITLE),
        "the section is absent: {texts:?}"
    );
    assert!(!texts.iter().any(|text| text == SHOW_ALL), "{texts:?}");
    assert!(
        section <= f32::EPSILON,
        "the page takes no room for a section: {section}"
    );
    assert!(block > 0.0, "the page keeps its block: {block}");
    assert!(
        texts.iter().any(|text| text == crate::app::welcome::HEADING),
        "the page keeps its heading: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text == crate::app::welcome::SHORTCUTS_LEAD),
        "the page keeps its row of shortcuts: {texts:?}"
    );
}

#[test]
fn the_section_adds_the_height_of_its_rows_to_the_block_of_the_page() {
    let (_temp, ctx, mut empty) = app_with_pickup(Vec::new());
    let _ = page_texts(&ctx, &mut empty);
    let (bare, _) = empty.welcome_block_heights_for_test(&ctx);
    let (_temp_two, ctx_two, mut full) = app_with_pickup(vec![folder(
        Path::new("/tmp/one"),
        "one",
        vec![ready_row(), ready_row()],
    )]);
    let _ = page_texts(&ctx_two, &mut full);
    let (block, section) = full.welcome_block_heights_for_test(&ctx_two);

    assert!(section > 0.0, "two rows take room: {section}");
    assert!(
        (block - bare - section).abs() < 0.01,
        "the block grows by the section alone: {block} against {bare} and {section}"
    );
}

#[test]
fn the_window_counts_the_rows_and_offers_the_press_of_a_row() {
    let (_temp, ctx, mut app) = app_with_pickup(vec![folder(Path::new("/tmp/one"), "one", vec![ready_row()])]);
    app.show_recent_agent_sessions();

    let texts = window_texts(&ctx, &mut app);

    assert!(texts.iter().any(|text| text == WINDOW_TITLE), "{texts:?}");
    assert!(
        texts.iter().any(|text| text == "1 session in 1 folder · Newest first"),
        "the window counts its rows: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text == OPEN),
        "a row offers its press: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text == "one"),
        "the folder group of the row: {texts:?}"
    );
}

#[test]
fn the_window_of_an_empty_pickup_says_that_it_has_no_session() {
    let (_temp, ctx, mut app) = app_with_pickup(Vec::new());
    app.show_recent_agent_sessions();

    let texts = window_texts(&ctx, &mut app);

    assert!(texts.iter().any(|text| text == EMPTY_LINE), "{texts:?}");
    assert!(!texts.iter().any(|text| text == OPEN), "no row offers a press");
}

#[test]
fn the_menu_row_opens_the_window_and_escape_closes_it() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();

    app.perform_toolbar_action(&ctx, ToolbarAction::RecentSessions);

    assert!(app.session_pickup.window_open(), "the Menu row opens the list");
    assert!(app.host_dialog_open(), "the window blocks the board under it");

    // The window takes a frame of its own before the toolkit knows it, so the
    // press of Escape reaches the second frame that shows it.
    for _ in 0..2 {
        let _ = ctx
            .run_ui(raw_input([1200.0, 900.0], None), |ui| {
                app.render_recent_agent_sessions(ui.ctx());
            })
            .discard_textures();
    }
    let mut escape = raw_input([1200.0, 900.0], None);
    escape.events.push(egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    let _ = ctx
        .run_ui(escape, |ui| app.render_recent_agent_sessions(ui.ctx()))
        .discard_textures();

    assert!(!app.session_pickup.window_open(), "Escape closes the window");
}

#[test]
fn a_row_that_another_program_has_open_asks_before_it_opens() {
    let folder_path = std::env::temp_dir().join("horizon-pickup-not-opened");
    let (_temp, ctx, mut app) = app_with_pickup(vec![folder(
        &folder_path,
        "one",
        vec![PickupRow {
            state: PickupState::OpenInAnotherProgram,
            ..ready_row()
        }],
    )]);

    app.apply_pickup_press(&ctx, &PickupPress::Open { folder: 0, row: 0 });

    assert!(
        app.board.panels.is_empty(),
        "the first press starts no panel while another program has the session"
    );
    assert!(app.session_pickup.is_confirmed(PanelKind::Claude, SESSION));
    let notice = app.session_pickup.notice.clone().expect("a notice that says why");
    assert!(notice.contains("open now"), "{notice}");
    assert!(notice.contains(OPEN_ANYWAY), "{notice}");
}

#[test]
fn a_row_whose_program_is_not_installed_never_asks_for_a_confirmation() {
    let folder_path = std::env::temp_dir().join("horizon-pickup-missing");
    let (_temp, ctx, mut app) = app_with_pickup(vec![folder(
        &folder_path,
        "one",
        vec![PickupRow {
            state: PickupState::ProgramMissing,
            ..ready_row()
        }],
    )]);

    app.apply_pickup_press(&ctx, &PickupPress::Open { folder: 0, row: 0 });

    assert!(app.board.panels.is_empty(), "a row that cannot open starts nothing");
    assert!(
        !app.session_pickup.is_confirmed(PanelKind::Claude, SESSION),
        "the row asks for no confirmation that it can never use"
    );
}

#[test]
fn the_open_of_a_row_hands_the_session_the_folder_and_the_preset_to_the_panel() {
    let temp = tempfile::tempdir().expect("temp dir");
    let folder_path = temp.path().join("repo");
    std::fs::create_dir_all(&folder_path).expect("the folder");
    let (_temp, _ctx, app) = app_with_pickup(Vec::new());
    let preset = app
        .presets
        .iter()
        .find(|preset| preset.kind == PanelKind::Claude)
        .cloned()
        .expect("a Claude preset of the default settings");

    let options = ready_row().panel_options(&folder_path, &preset, &app.template_config.browser);

    assert_eq!(options.kind, PanelKind::Claude);
    assert_eq!(
        options.resume,
        horizon_core::PanelResume::Session {
            session_id: SESSION.to_string()
        },
        "the panel resumes exactly this session"
    );
    assert_eq!(options.cwd.as_deref(), Some(folder_path.as_path()));
    assert_eq!(options.name.as_deref(), Some("Fix the build"));
    assert_eq!(options.command, preset.command, "the program of the preset");
    assert_eq!(options.args, preset.args, "the arguments of the preset");
}

#[test]
fn the_open_of_a_row_without_a_preset_of_its_program_says_where_to_add_one() {
    let temp = tempfile::tempdir().expect("temp dir");
    let folder_path = temp.path().join("repo");
    std::fs::create_dir_all(&folder_path).expect("the folder");
    let (_temp, ctx, mut app) = app_with_pickup(Vec::new());
    app.presets.retain(|preset| preset.kind != PanelKind::Claude);
    let row = ready_row();

    let opened = app.open_agent_session(&ctx, &folder_path, &row);

    assert!(!opened, "no preset of that program, so no panel");
    assert!(app.board.panels.is_empty());
    assert!(
        app.board.workspaces.is_empty(),
        "no workspace is made for a panel that cannot open"
    );
    let notice = app.session_pickup.notice.clone().expect("a notice");
    assert!(notice.contains("Settings, Presets"), "{notice}");
}

#[test]
fn the_workspace_of_a_folder_is_made_once_and_named_after_the_folder() {
    let temp = tempfile::tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let other = temp.path().join("other");
    std::fs::create_dir_all(&repo).expect("the folder");
    std::fs::create_dir_all(&other).expect("the folder");
    let (_temp, ctx, mut app) = app_with_pickup(Vec::new());

    let first = app.ensure_workspace_of_folder(&ctx, &repo);
    let again = app.ensure_workspace_of_folder(&ctx, &repo);
    let second = app.ensure_workspace_of_folder(&ctx, &other);

    assert_eq!(first, again, "a second call uses the workspace that is there");
    assert_ne!(first, second);
    assert_eq!(app.board.workspaces.len(), 2, "one workspace of each folder");
    let workspace = app.board.workspace(first).expect("the workspace");
    assert_eq!(workspace.name, "repo");
    assert_eq!(workspace.cwd.as_deref(), Some(repo.as_path()));
    assert_eq!(app.board.active_workspace, Some(second), "the view shows the workspace");
}

#[test]
fn a_folder_of_the_board_is_found_by_a_path_that_resolves_to_it() {
    let temp = tempfile::tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("the folder");
    let (_temp, _ctx, mut app) = app_with_pickup(Vec::new());
    let workspace = app.board.create_workspace("repo");
    app.board.workspace_mut(workspace).expect("workspace").cwd = Some(repo.clone());

    let spelled = PathBuf::from(format!("{}/", repo.display()));

    assert_eq!(app.workspace_of_folder(&spelled), Some(workspace));
    assert_eq!(app.workspace_of_folder(&temp.path().join("elsewhere")), None);
}

#[test]
fn the_window_names_every_folder_and_its_rows() {
    let folders = vec![
        folder(
            Path::new("/tmp/one"),
            "one",
            vec![
                ready_row(),
                PickupRow {
                    state: PickupState::OpenInAnotherProgram,
                    ..ready_row()
                },
            ],
        ),
        folder(Path::new("/tmp/two"), "two", Vec::new()),
    ];
    let list = PickupList::new(folders.clone());

    assert_eq!(summary_line(&list, &folders), "2 sessions in 2 folders · Newest first");
    assert_eq!(folders[0].open_all_count(), 2, "the folder opens both rows at once");
    assert_eq!(folder_text(&folders[1]), "two in ~/github");
    assert!(window_body_height(&folders) > 2.0 * 44.0);
    assert!(EMPTY_LINE.contains("No conversation"));
    assert_eq!(MENU_TITLE, WINDOW_TITLE, "the Menu names the window that it opens");
    assert_eq!(OPEN_ALL, "Open all");
}

#[test]
fn open_all_leaves_a_row_that_waits_for_a_confirmation_in_the_list() {
    let folders = folder(
        Path::new("/tmp/one"),
        "one",
        vec![
            ready_row(),
            PickupRow {
                session_id: OTHER_SESSION.to_string(),
                state: PickupState::OpenInAnotherProgram,
                ..ready_row()
            },
            PickupRow {
                session_id: "00000000-0000-0000-0000-000000000003".to_string(),
                state: PickupState::FolderGone,
                ..ready_row()
            },
        ],
    );

    let chosen = super::open_all_rows(&folders);

    assert_eq!(chosen.len(), 1, "one row of the folder opens at one press: {chosen:?}");
    assert_eq!(chosen[0].session_id, SESSION);
}

#[test]
fn the_pickup_of_this_window_is_read_from_the_stores_on_a_worker() {
    let (temp, ctx, mut app) = app_with_pickup(Vec::new());
    // The agent programs keep their stores in the home of the person. The home of
    // Horizon is another folder, and a store that stands there is no store of an
    // agent program: the pickup must not read it.
    let home = temp.path().join("home-of-the-person");
    let horizon_home = app.session_store.home().root().to_path_buf();
    assert_ne!(home, horizon_home);
    let folder = home.join("repo");
    std::fs::create_dir_all(&folder).expect("the folder");
    let transcript = |store_home: &Path, session: &str, prompt: &str| {
        let project = store_home.join(".claude").join("projects").join("-repo");
        std::fs::create_dir_all(&project).expect("the Claude store");
        let record = serde_json::json!({
            "type": "user",
            "sessionId": session,
            "cwd": folder.display().to_string(),
        });
        let prompt = serde_json::json!({"type": "last-prompt", "lastPrompt": prompt});
        std::fs::write(
            project.join(format!("{session}.jsonl")),
            format!("{record}\n{prompt}\n"),
        )
        .expect("the transcript");
    };
    transcript(&home, SESSION, "Pick this one up");
    transcript(&horizon_home, OTHER_SESSION, "A file in the home of Horizon");
    app.session_pickup = SessionPickup::reading(home);

    // The window reads the store on its worker, so the answer comes after a frame
    // or two. The whole suite runs under the strict guard of the UI thread, so a
    // read or a stat on the frame ends this test.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        run_app_frame(&ctx, &mut app);
        if !app.session_pickup.first_page().is_empty() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the pickup read the store");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let page = app.session_pickup.first_page();
    assert_eq!(page.len(), 1);
    assert_eq!(
        page[0].rows.len(),
        1,
        "the home of Horizon holds no store of an agent program"
    );
    assert_eq!(page[0].rows[0].title, "Pick this one up");
    assert_eq!(page[0].rows[0].session_id, SESSION);
    assert_eq!(page[0].name, "repo");
}

#[test]
fn a_row_names_its_folder_and_the_program() {
    let (title, detail) = row_lines(&ready_row(), Some(&folder(Path::new("/tmp/one"), "one", Vec::new())));

    assert_eq!(title, "Fix the build");
    assert_eq!(detail, "Claude · one in ~/github · 2 hours ago");
    assert_eq!(folder_name(Path::new("/tmp/my repo")), "my repo");
    assert_eq!(open_label(true), OPEN_ANYWAY);
    assert!(spoken(&ready_row()).starts_with("Fix the build. Claude"));
}

#[test]
fn the_programs_of_the_pickup_are_the_programs_of_the_presets() {
    let presets = vec![
        PresetConfig {
            name: "Claude Code".to_string(),
            alias: None,
            kind: PanelKind::Claude,
            command: None,
            args: Vec::new(),
            resume: horizon_core::PanelResume::Fresh,
            ssh_connection: None,
        },
        PresetConfig {
            name: "Shell".to_string(),
            alias: None,
            kind: PanelKind::Shell,
            command: None,
            args: Vec::new(),
            resume: horizon_core::PanelResume::Fresh,
            ssh_connection: None,
        },
    ];

    let programs = PickupProgram::from_presets(&presets);

    assert_eq!(programs.len(), 1);
    assert_eq!(programs[0].kind, PanelKind::Claude);
    assert_eq!(programs[0].program, "claude");
}

/// The Claude store of `home` holds `session` in `folder`, as that program writes it.
fn transcript_of(home: &Path, folder: &Path, session: &str) {
    let project = home.join(".claude").join("projects").join("-repo");
    std::fs::create_dir_all(&project).expect("the Claude store");
    let record = serde_json::json!({
        "type": "user",
        "sessionId": session,
        "cwd": folder.display().to_string(),
    });
    let prompt = serde_json::json!({"type": "last-prompt", "lastPrompt": "Pick this one up"});
    std::fs::write(
        project.join(format!("{session}.jsonl")),
        format!("{record}\n{prompt}\n"),
    )
    .expect("the transcript");
}

#[test]
fn a_session_of_an_open_panel_is_not_a_row_of_the_list() {
    let temp = tempfile::tempdir().expect("temp dir");
    let home = temp.path().join("home-of-the-person");
    let folder_path = temp.path().join("repo");
    std::fs::create_dir_all(&folder_path).expect("the folder");
    transcript_of(&home, &folder_path, SESSION);
    let programs = vec![PickupProgram {
        kind: PanelKind::Claude,
        program: "claude".to_string(),
    }];
    let open: HashSet<AgentSessionKey> = HashSet::new();
    let list = horizon_core::session_pickup::load_with(&home, &programs, &open, 0, |_| true);
    assert_eq!(list.row_count(), 1, "the store of that home names one conversation");

    let open = HashSet::from([AgentSessionKey::new(PanelKind::Claude, SESSION)]);
    let list = horizon_core::session_pickup::load_with(&home, &programs, &open, 0, |_| true);

    assert!(
        list.is_empty(),
        "the pickup leaves out the sessions that the caller names"
    );
    assert_eq!(
        list.open_in_panels, 1,
        "the read names the session that an open panel has"
    );
}

#[test]
fn the_window_says_that_every_session_of_this_computer_is_open_in_a_panel() {
    let (temp, ctx, mut app) = app_with_pickup(Vec::new());
    let home = temp.path().join("home-of-the-person");
    let folder_path = home.join("repo");
    std::fs::create_dir_all(&folder_path).expect("the folder");
    transcript_of(&home, &folder_path, SESSION);
    // A panel of this board has the one conversation of that store.
    let workspace = app.board.create_workspace("a");
    app.board
        .create_panel(
            horizon_core::PanelOptions {
                kind: PanelKind::Claude,
                command: Some("seed".to_string()),
                resume: horizon_core::PanelResume::Session {
                    session_id: SESSION.to_string(),
                },
                ..horizon_core::PanelOptions::default()
            },
            workspace,
        )
        .expect("the panel of the session");
    app.session_pickup = SessionPickup::reading(home);
    app.show_recent_agent_sessions();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while app.session_pickup.pending.is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "the pickup reads the store of the home"
        );
        run_app_frame(&ctx, &mut app);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let texts = window_texts(&ctx, &mut app);

    assert!(
        texts.iter().any(|text| text == EVERY_ONE_OPEN_LINE),
        "the window names the conversations that panels have: {texts:?}"
    );
    assert!(!texts.iter().any(|text| text == EMPTY_LINE), "{texts:?}");
}

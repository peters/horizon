//! The pickup list: from the files of a session store to the rows of a page.
//!
//! Every store of a test is made here, in the home of the test. No file of the
//! home of a person is read.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::{
    MAX_FIRST_PAGE_ROWS, MAX_OPEN_AT_ONCE, MAX_SESSION_AGE_DAYS, MAX_SESSION_ROWS, MAX_TITLE_CHARS, PickupList,
    PickupProgram, PickupState, load_with, reads_sessions,
};
use crate::config::PresetConfig;
use crate::panel::{PanelKind, PanelResume};
use crate::runtime_state::AgentSessionKey;

const HOUR_MILLIS: i64 = 60 * 60 * 1000;
const DAY_MILLIS: i64 = 24 * HOUR_MILLIS;
const NOW: i64 = 1_700_000_000_000;

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path().join("home");
        std::fs::create_dir_all(home.join(".claude/projects")).expect("the Claude store");
        std::fs::create_dir_all(home.join(".claude/sessions")).expect("the live registry");
        std::fs::create_dir_all(home.join(".codex")).expect("the Codex store");
        Self { _temp: temp, home }
    }

    fn home(&self) -> &Path {
        &self.home
    }

    /// The folder of a project, made on disk.
    fn folder(&self, name: &str) -> PathBuf {
        let folder = self.home.join(name);
        std::fs::create_dir_all(&folder).expect("the folder of a project");
        folder
    }

    /// One Claude Code transcript, as that program writes it.
    fn claude_session(&self, folder: &Path, session_id: &str, prompt: &str, updated_at: i64) {
        let project = self.home.join(".claude/projects").join("-project");
        std::fs::create_dir_all(&project).expect("the project folder");
        let record = json!({
            "type": "user",
            "sessionId": session_id,
            "cwd": folder.display().to_string(),
            "slug": "a-slug",
        });
        let last_prompt = json!({"type": "last-prompt", "lastPrompt": prompt});
        let path = project.join(format!("{}.jsonl", safe_file_stem(session_id)));
        std::fs::write(&path, format!("{record}\n{last_prompt}\n")).expect("the transcript");
        set_modified(&path, updated_at);
    }

    /// One Codex session row, in the store of that program.
    fn codex_session(&self, folder: &Path, session_id: &str, title: &str, updated_at: i64) {
        let store = self.home.join(".codex").join("state_5.sqlite");
        let connection = rusqlite::Connection::open(&store).expect("the Codex store");
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS threads (
                    id TEXT PRIMARY KEY,
                    rollout_path TEXT,
                    source TEXT NOT NULL,
                    title TEXT,
                    cwd TEXT,
                    updated_at INTEGER,
                    archived INTEGER NOT NULL DEFAULT 0
                );",
            )
            .expect("the table of the Codex store");
        connection
            .execute(
                "INSERT OR REPLACE INTO threads (id, rollout_path, source, title, cwd, updated_at, archived)
                 VALUES (?1, '', 'cli', ?2, ?3, ?4, 0)",
                rusqlite::params![session_id, title, folder.display().to_string(), updated_at / 1000],
            )
            .expect("the row of the Codex store");
    }

    /// The registry entry of a Claude Code process that runs now.
    fn live_claude_process(&self, pid: u32, session_id: &str) {
        std::fs::write(
            self.home.join(".claude/sessions").join(format!("{pid}.json")),
            json!({"pid": pid, "sessionId": session_id, "cwd": "/repo"}).to_string(),
        )
        .expect("the registry entry");
    }
}

/// The name of a transcript file for `session_id`.
///
/// A store of another program names its files itself, and a test also writes an
/// id that could not be a file name of one folder, so the file takes the safe
/// characters of the id. The id that a row must check is the one inside the
/// record, not the name of the file.
fn safe_file_stem(session_id: &str) -> String {
    let stem: String = session_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if stem.is_empty() { "session".to_string() } else { stem }
}

fn set_modified(path: &Path, millis: i64) {
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_millis(u64::try_from(millis).expect("a time"));
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("the file")
        .set_modified(time)
        .expect("the time of the file");
}

fn programs() -> Vec<PickupProgram> {
    vec![
        PickupProgram {
            kind: PanelKind::Claude,
            program: "claude".to_string(),
        },
        PickupProgram {
            kind: PanelKind::Codex,
            program: "codex".to_string(),
        },
    ]
}

fn load(list_home: &Path, open_panels: &HashSet<AgentSessionKey>) -> PickupList {
    load_with(list_home, &programs(), open_panels, NOW, |_| true)
}

#[test]
fn the_two_programs_of_this_round_have_a_store_and_the_others_do_not() {
    assert!(reads_sessions(PanelKind::Claude));
    assert!(reads_sessions(PanelKind::Codex));
    for kind in [
        PanelKind::Gemini,
        PanelKind::KiloCode,
        PanelKind::OpenCode,
        PanelKind::Pi,
        PanelKind::Grok,
        PanelKind::Shell,
        PanelKind::Ssh,
        PanelKind::Command,
        PanelKind::Editor,
        PanelKind::Browser,
        PanelKind::Device,
        PanelKind::Block,
    ] {
        assert!(!reads_sessions(kind), "kind {kind:?}");
    }
    assert_eq!(
        super::READ_PROGRAMS,
        [PanelKind::Claude, PanelKind::Codex],
        "the boundary names these two programs"
    );
}

#[test]
fn the_programs_of_the_presets_are_the_two_that_resume_one_conversation() {
    let presets = vec![
        preset("Shell", PanelKind::Shell, None),
        preset("Claude", PanelKind::Claude, None),
        preset("Gemini", PanelKind::Gemini, None),
        preset("Kilo", PanelKind::KiloCode, None),
        preset("Codex", PanelKind::Codex, Some("codex --full-auto")),
        preset("Claude again", PanelKind::Claude, None),
    ];

    let programs = PickupProgram::from_presets(&presets);

    assert_eq!(programs.len(), 2, "one program for each kind: {programs:?}");
    assert_eq!(programs[0].kind, PanelKind::Claude);
    assert_eq!(programs[0].program, "claude", "the default command of the kind");
    assert_eq!(programs[1].kind, PanelKind::Codex);
    assert_eq!(
        programs[1].program, "codex",
        "the command of the preset without its arguments"
    );
}

#[test]
fn both_stores_answer_rows_grouped_by_their_folder() {
    let fixture = Fixture::new();
    let first = fixture.folder("repo-one");
    let second = fixture.folder("repo-two");
    fixture.claude_session(&first, &uuid(1), "Fix the build", NOW - 2 * HOUR_MILLIS);
    fixture.codex_session(&second, &uuid(2), "Write the test", NOW - HOUR_MILLIS);
    fixture.codex_session(&first, &uuid(3), "Another session", NOW - 30 * HOUR_MILLIS);

    let list = load(fixture.home(), &HashSet::new());

    assert_eq!(list.folders().len(), 2);
    let newest = &list.folders()[0];
    assert_eq!(newest.path, std::fs::canonicalize(&second).expect("the folder"));
    assert_eq!(newest.name, "repo-two");
    assert_eq!(newest.rows.len(), 1);
    assert_eq!(newest.rows[0].kind, PanelKind::Codex);
    assert_eq!(newest.rows[0].title, "Write the test");
    assert_eq!(newest.rows[0].last_active, "an hour ago");
    assert_eq!(newest.rows[0].state, PickupState::Ready);
    assert_eq!(newest.rows[0].session_id, uuid(2));

    let older = &list.folders()[1];
    assert_eq!(older.name, "repo-one");
    assert_eq!(older.rows.len(), 2);
    assert_eq!(
        older.rows.iter().map(|row| row.title.as_str()).collect::<Vec<_>>(),
        ["Fix the build", "Another session"],
        "the newest row of a folder comes first"
    );
    assert_eq!(older.rows[1].last_active, "yesterday");
}

#[test]
fn a_title_of_a_normal_length_is_kept_whole() {
    // One sentence of a page: the words that an agent writes as the prompt of a
    // session, and that the row of a wide window holds whole.
    let prompt = "Write the page about the first start and keep every sentence short enough for the \
                  row that shows it, then read the page again";
    let fixture = Fixture::new();
    let folder = fixture.folder("repo");
    fixture.claude_session(&folder, &uuid(1), prompt, NOW);

    let list = load(fixture.home(), &HashSet::new());
    let title = &list.folders()[0].rows[0].title;

    assert_eq!(
        title,
        prompt,
        "the title of a row of a normal length is kept whole, with {} characters",
        prompt.chars().count()
    );
}

#[test]
fn a_row_shows_the_title_of_the_program_and_never_more_than_one_line() {
    let fixture = Fixture::new();
    let folder = fixture.folder("repo");
    fixture.claude_session(
        &folder,
        &uuid(1),
        &format!("line one\u{1b}[2J\nline two\t{}", "x".repeat(5000)),
        NOW,
    );

    let list = load(fixture.home(), &HashSet::new());
    let title = &list.folders()[0].rows[0].title;

    assert!(
        !title.contains('\n') && !title.contains('\u{1b}') && !title.contains('\t'),
        "{title}"
    );
    assert!(title.chars().count() <= MAX_TITLE_CHARS, "{}", title.chars().count());
    assert!(title.starts_with("line one [2J line two"), "{title}");
}

#[test]
fn a_session_without_a_title_names_its_program() {
    let fixture = Fixture::new();
    let folder = fixture.folder("repo");
    fixture.codex_session(&folder, &uuid(1), "", NOW);

    let list = load(fixture.home(), &HashSet::new());

    assert_eq!(list.folders()[0].rows[0].title, "Codex session");
}

#[test]
fn a_folder_that_is_gone_says_so_and_cannot_open() {
    let fixture = Fixture::new();
    let folder = fixture.home().join("gone");
    std::fs::create_dir_all(&folder).expect("the folder");
    fixture.claude_session(&folder, &uuid(1), "A session", NOW);
    std::fs::remove_dir_all(&folder).expect("the folder goes");

    let list = load(fixture.home(), &HashSet::new());
    let folder_row = &list.folders()[0];

    assert!(!folder_row.exists);
    assert_eq!(folder_row.rows[0].state, PickupState::FolderGone);
    assert_eq!(folder_row.rows[0].state.says(), Some("folder is gone"));
    assert!(!folder_row.rows[0].state.opens(), "the row cannot open");
}

#[test]
fn a_cwd_that_is_a_file_is_not_a_folder() {
    let fixture = Fixture::new();
    let file = fixture.home().join("a-file");
    std::fs::write(&file, b"not a folder").expect("the file");
    fixture.claude_session(&file, &uuid(1), "A session", NOW);

    let list = load(fixture.home(), &HashSet::new());

    assert_eq!(list.folders()[0].rows[0].state, PickupState::FolderGone);
}

#[test]
fn a_folder_with_a_space_in_its_name_keeps_the_name_it_has() {
    let fixture = Fixture::new();
    let folder = fixture.folder("my repo");
    fixture.codex_session(&folder, &uuid(1), "A session", NOW);

    let list = load(fixture.home(), &HashSet::new());

    assert_eq!(list.folders()[0].name, "my repo");
    assert_eq!(
        list.folders()[0].path,
        std::fs::canonicalize(&folder).expect("the folder")
    );
}

#[test]
fn a_session_id_that_is_not_of_the_form_of_the_program_is_never_a_row() {
    let fixture = Fixture::new();
    let folder = fixture.folder("repo");
    for (index, session_id) in [
        "x; rm -rf ~",
        "$(touch pwned)",
        "../../etc/passwd",
        "",
        "00000000-0000-0000-0000-00000000000Z",
        "0123456789abcdef0123456789abcdef0123",
    ]
    .into_iter()
    .enumerate()
    {
        let age = i64::try_from(index).expect("an index of a session");
        fixture.claude_session(&folder, session_id, &format!("A session {index}"), NOW - age);
    }
    fixture.codex_session(&folder, "00000000-0000-0000-0000-00000000000z", "Codex one", NOW);

    let list = load(fixture.home(), &HashSet::new());

    assert!(list.is_empty(), "no row offers an id that is not a UUID: {list:?}");
}

#[test]
fn a_session_older_than_the_named_number_of_days_is_not_a_row() {
    let fixture = Fixture::new();
    let folder = fixture.folder("repo");
    fixture.claude_session(
        &folder,
        &uuid(1),
        "Fresh",
        NOW - (MAX_SESSION_AGE_DAYS - 1) * DAY_MILLIS,
    );
    fixture.claude_session(
        &folder,
        &uuid(2),
        "Stale",
        NOW - (MAX_SESSION_AGE_DAYS + 1) * DAY_MILLIS,
    );

    let list = load(fixture.home(), &HashSet::new());

    assert_eq!(list.row_count(), 1, "one row of the two: {list:?}");
    assert_eq!(list.folders()[0].rows[0].title, "Fresh");
}

#[test]
fn the_list_holds_only_the_named_number_of_rows_of_a_large_store() {
    let fixture = Fixture::new();
    let folder = fixture.folder("repo");
    for index in 0..5000_i64 {
        fixture.claude_session(
            &folder,
            &uuid(u64::try_from(index).expect("an index") + 1),
            &format!("Session {index}"),
            NOW - index,
        );
    }

    let list = load(fixture.home(), &HashSet::new());

    assert_eq!(list.row_count(), MAX_SESSION_ROWS, "the rows of the page are bounded");
    assert_eq!(list.folders()[0].rows[0].title, "Session 0", "the newest row is first");
    assert_eq!(
        list.folders()[0].rows[MAX_SESSION_ROWS - 1].title,
        format!("Session {}", MAX_SESSION_ROWS - 1)
    );
}

#[test]
fn a_session_that_an_open_panel_has_is_not_offered_again() {
    let fixture = Fixture::new();
    let folder = fixture.folder("repo");
    fixture.claude_session(&folder, &uuid(1), "Taken", NOW);
    fixture.claude_session(&folder, &uuid(2), "Free", NOW - HOUR_MILLIS);
    let open: HashSet<AgentSessionKey> = HashSet::from([AgentSessionKey::new(PanelKind::Claude, uuid(1))]);

    let list = load(fixture.home(), &open);

    assert_eq!(list.row_count(), 1);
    assert_eq!(list.folders()[0].rows[0].title, "Free");
}

#[test]
fn a_session_that_another_process_has_open_is_marked_and_asks_first() {
    let fixture = Fixture::new();
    let folder = fixture.folder("repo");
    fixture.claude_session(&folder, &uuid(1), "Running", NOW);
    fixture.live_claude_process(std::process::id(), &uuid(1));
    // A registry entry of a process that has ended keeps no mark on a row.
    fixture.claude_session(&folder, &uuid(2), "Ended", NOW - HOUR_MILLIS);
    std::fs::write(
        fixture.home().join(".claude/sessions").join("4294967.json"),
        json!({"pid": 4_294_967, "sessionId": uuid(2)}).to_string(),
    )
    .expect("the registry entry of a process that ended");

    let list = load(fixture.home(), &HashSet::new());
    let rows = &list.folders()[0].rows;

    assert_eq!(rows[0].state, PickupState::OpenInAnotherProgram);
    assert_eq!(rows[0].state.says(), Some("open in another program"));
    assert!(rows[0].state.asks_first(), "the row asks for a second confirmation");
    assert!(rows[0].state.opens(), "the row still opens after that confirmation");
    assert_eq!(rows[1].state, PickupState::Ready);
}

#[test]
fn a_program_that_is_not_installed_says_so_and_cannot_open() {
    let fixture = Fixture::new();
    let folder = fixture.folder("repo");
    fixture.claude_session(&folder, &uuid(1), "Claude one", NOW);
    fixture.codex_session(&folder, &uuid(2), "Codex one", NOW - HOUR_MILLIS);

    let list = load_with(fixture.home(), &programs(), &HashSet::new(), NOW, |program| {
        program == "codex"
    });
    let rows = &list.folders()[0].rows;

    let claude = rows
        .iter()
        .find(|row| row.kind == PanelKind::Claude)
        .expect("the Claude row");
    assert_eq!(claude.state, PickupState::ProgramMissing);
    assert_eq!(claude.state.says(), Some("program is not installed"));
    assert!(!claude.state.opens());
    let codex = rows
        .iter()
        .find(|row| row.kind == PanelKind::Codex)
        .expect("the Codex row");
    assert_eq!(codex.state, PickupState::Ready);
}

#[test]
fn the_first_page_holds_the_newest_row_of_six_folders_and_the_list_holds_all_of_them() {
    let fixture = Fixture::new();
    for index in 0..8_i64 {
        let folder = fixture.folder(&format!("repo-{index}"));
        fixture.claude_session(
            &folder,
            &uuid(u64::try_from(index).expect("an index") + 1),
            &format!("Session {index}"),
            NOW - index * HOUR_MILLIS,
        );
        // A second session of the same folder is not a second row of the page.
        fixture.claude_session(
            &folder,
            &uuid(u64::try_from(index).expect("an index") + 100),
            &format!("Older {index}"),
            NOW - index * HOUR_MILLIS - 60_000,
        );
    }

    let list = load(fixture.home(), &HashSet::new());
    let page = list.first_page();

    assert_eq!(list.folders().len(), 8);
    assert_eq!(page.len(), MAX_FIRST_PAGE_ROWS);
    assert_eq!(page[0].rows.len(), 1, "one row of each folder of the page");
    assert_eq!(page[0].rows[0].title, "Session 0");
    assert_eq!(
        page.iter().map(|folder| folder.name.as_str()).collect::<Vec<_>>(),
        ["repo-0", "repo-1", "repo-2", "repo-3", "repo-4", "repo-5"]
    );
}

#[test]
fn a_folder_opens_all_of_its_rows_up_to_the_named_number() {
    let fixture = Fixture::new();
    let folder = fixture.folder("repo");
    for index in 0..6_i64 {
        fixture.claude_session(
            &folder,
            &uuid(u64::try_from(index).expect("an index") + 1),
            &format!("Session {index}"),
            NOW - index * 60_000,
        );
    }

    let list = load(fixture.home(), &HashSet::new());

    assert_eq!(list.folders()[0].open_all_count(), MAX_OPEN_AT_ONCE);
    let single = PickupList {
        folders: vec![super::PickupFolder {
            rows: vec![list.folders()[0].rows[0].clone()],
            ..list.folders()[0].clone()
        }],
        ..PickupList::new(Vec::new())
    };
    assert_eq!(single.folders()[0].open_all_count(), 0, "one row opens by itself");
}

#[test]
fn a_store_that_is_not_there_answers_no_row() {
    let temp = tempfile::tempdir().expect("temp dir");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).expect("the home");

    let list = load(&home, &HashSet::new());

    assert!(list.is_empty(), "a person who never ran an agent sees no row");
    assert!(list.first_page().is_empty());
}

#[test]
fn the_rows_answer_the_session_and_the_folder_of_one_row() {
    let fixture = Fixture::new();
    let folder = fixture.folder("repo");
    fixture.claude_session(&folder, &uuid(1), "A session", NOW);

    let list = load(fixture.home(), &HashSet::new());

    assert!(list.row(PanelKind::Claude, &uuid(1)).is_some());
    assert!(list.row(PanelKind::Codex, &uuid(1)).is_none());
    assert_eq!(
        list.folder_of(PanelKind::Claude, &uuid(1))
            .map(|folder| folder.name.as_str()),
        Some("repo")
    );
    assert_eq!(
        list.row(PanelKind::Claude, &uuid(1)).map(super::PickupRow::key),
        Some(AgentSessionKey::new(PanelKind::Claude, uuid(1)))
    );
}

fn preset(name: &str, kind: PanelKind, command: Option<&str>) -> PresetConfig {
    PresetConfig {
        name: name.to_string(),
        alias: None,
        kind,
        command: command.map(str::to_string),
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: None,
    }
}

/// The UUID of `value`, so a test names a session with the form that both
/// programs use.
fn uuid(value: u64) -> String {
    format!("00000000-0000-0000-0000-{value:012x}")
}

#[test]
fn the_open_of_a_row_starts_the_program_of_the_row_with_exactly_its_session() {
    let fixture = Fixture::new();
    let folder = fixture.folder("repo");
    fixture.claude_session(&folder, &uuid(1), "Fix the build", NOW);
    let list = load(fixture.home(), &HashSet::new());
    let folder_row = &list.folders()[0];
    let row = &folder_row.rows[0];
    let preset = preset("Claude Code", PanelKind::Claude, None);

    let options = row.panel_options(&folder_row.path, &preset, &crate::browser::BrowserConfig::default());
    let start = crate::panel::prepare_panel_start(crate::panel::StartRequest {
        id: crate::panel::PanelId(7),
        opts: options,
        local_id: "pickup-panel".to_string(),
        home: crate::HorizonHome::resolve(),
        keep_agents_through_restart: false,
    });

    let (program, args) = start.launch_command().expect("the launch of the program");
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    assert_eq!(
        program, shell,
        "an agent panel starts through the login shell of this computer"
    );
    assert_eq!(args.len(), 2, "{args:?}");
    assert_eq!(args[0], "-ic", "the shell runs one command string");
    // A panel resumes a conversation whose transcript is in the store of this
    // computer, and starts the same program under the same id when the store has
    // no transcript of it: the rule of `docs/session-recovery.md`. A test home is
    // not the home that the launch reads, so both spellings are correct here.
    let id = uuid(1);
    let resume = format!("claude --resume {id}");
    let fresh = format!("claude --session-id {id}");
    assert!(
        args[1] == resume || args[1] == fresh,
        "the command string names the program and the one session, and nothing else: {:?}",
        args[1]
    );
    assert_eq!(
        args[1].matches(&id).count(),
        1,
        "the session id stands once: {:?}",
        args[1]
    );
}

#[test]
fn a_codex_row_resumes_with_the_subcommand_of_its_program() {
    let fixture = Fixture::new();
    let folder = fixture.folder("repo");
    fixture.codex_session(&folder, &uuid(2), "Write the test", NOW);
    let list = load(fixture.home(), &HashSet::new());
    let folder_row = &list.folders()[0];
    let row = &folder_row.rows[0];
    let preset = preset("Codex", PanelKind::Codex, None);

    let options = row.panel_options(&folder_row.path, &preset, &crate::browser::BrowserConfig::default());
    let start = crate::panel::prepare_panel_start(crate::panel::StartRequest {
        id: crate::panel::PanelId(8),
        opts: options,
        local_id: "pickup-codex".to_string(),
        home: crate::HorizonHome::resolve(),
        keep_agents_through_restart: false,
    });

    let (_, args) = start.launch_command().expect("the launch of the program");
    assert!(
        args[1].contains(&format!("resume {}", uuid(2))),
        "the command string resumes the one session: {:?}",
        args[1]
    );
}

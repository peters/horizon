//! The reader of the session mirror, against hand-written stores.
//!
//! Every fixture here is made up: the text of a session of a person never enters
//! this repository, and this computer's stores are only read to find the shape of
//! a line.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use super::{
    MAX_LIST_SESSIONS, MAX_SUMMARY_CHARS, MAX_TEXT_CHARS, MirrorEntry, MirrorProgram, PROGRAMS, UNREAD_NOTICE,
    read_entries, read_list,
};

const DAY: i64 = 24 * 60 * 60 * 1_000;

/// The clock of the computer that runs the test, which the age bound counts from.
fn now() -> i64 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after the epoch");
    i64::try_from(since.as_millis()).expect("a time in milliseconds")
}

fn home() -> tempfile::TempDir {
    tempfile::tempdir().expect("a home for the test")
}

/// A Claude session id, so a fixture has the form that the program writes.
fn claude_id(index: u32) -> String {
    format!("00000000-0000-0000-0000-{index:012}")
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the folder of the fixture");
    }
    std::fs::write(path, text).expect("the fixture");
}

/// One transcript line of the person with a text.
fn person_line(text: &str) -> String {
    serde_json::json!({"type": "user", "cwd": "/repo", "message": {"role": "user", "content": text}}).to_string()
}

/// One transcript line of the agent with a text.
fn agent_line(id: &str, text: &str) -> String {
    serde_json::json!({
        "type": "assistant",
        "message": {"id": id, "role": "assistant", "content": [{"type": "text", "text": text}]},
    })
    .to_string()
}

/// The transcript of one Claude session of `home`, and its path.
fn claude_transcript(home: &Path, id: &str, lines: &[String]) -> PathBuf {
    let path = home.join(".claude/projects/-repo").join(format!("{id}.jsonl"));
    let mut text = String::new();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    write(&path, &text);
    path
}

/// The live registry entry of one Claude session.
fn claude_live(home: &Path, pid: u32, id: &str) {
    let entry = serde_json::json!({"pid": pid, "sessionId": id}).to_string();
    write(&home.join(".claude/sessions").join(format!("{pid}.json")), &entry);
}

/// The Codex store of one session, in the database and in its rollout.
fn codex_session(home: &Path, id: &str, cwd: &str, updated_at: i64) -> PathBuf {
    let rollout = home
        .join(".codex/sessions/2026/01/02")
        .join(format!("rollout-2026-01-02T00-00-00-{id}.jsonl"));
    write(
        &rollout,
        &format!(
            "{}\n{}\n",
            serde_json::json!({"type": "session_meta", "payload": {"id": id}}),
            serde_json::json!({"type": "response_item", "payload": {"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": "made-up request"}]}}),
        ),
    );
    let database = home.join(".codex/state_5.sqlite");
    let connection = Connection::open(&database).expect("the Codex store");
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS threads (
                id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, source TEXT NOT NULL, title TEXT NOT NULL,
                cwd TEXT NOT NULL, updated_at INTEGER NOT NULL, archived INTEGER NOT NULL DEFAULT 0
            );",
        )
        .expect("the threads table");
    connection
        .execute(
            "INSERT INTO threads (id, rollout_path, source, title, cwd, updated_at, archived)
             VALUES (?1, ?2, 'cli', ?3, ?4, ?5, 0)",
            params![
                id,
                rollout.display().to_string(),
                format!("made-up title {id}"),
                cwd,
                updated_at / 1_000
            ],
        )
        .expect("one thread");
    rollout
}

/// The Pi conversation of one session of `home`.
fn pi_session(home: &Path, id: &str, cwd: &str) -> PathBuf {
    let path = home
        .join(".pi/agent/sessions/--repo--")
        .join(format!("2026-01-02T00-00-00-000Z_{id}.jsonl"));
    write(
        &path,
        &format!(
            "{}\n{}\n",
            serde_json::json!({"type": "session", "id": id, "cwd": cwd}),
            serde_json::json!({"type": "message", "message": {"role": "user",
                "content": [{"type": "text", "text": "made-up request"}]}}),
        ),
    );
    path
}

#[test]
fn the_list_holds_the_sessions_of_every_program_that_this_build_reads() {
    let home = home();
    let claude = claude_id(1);
    claude_transcript(home.path(), &claude, &[person_line("made-up request")]);
    let codex = "0199aaaa-0000-7000-8000-000000000002";
    codex_session(home.path(), codex, "/repo", now() - DAY);
    let pi = "01a17ba5-0000-7000-8000-000000000003";
    pi_session(home.path(), pi, "/repo");

    let catalog = read_list(home.path(), now(), &|_| false);

    let kinds: Vec<MirrorProgram> = catalog.rows().iter().map(|row| row.session.program).collect();
    assert!(kinds.contains(&MirrorProgram::Claude), "{kinds:?}");
    assert!(kinds.contains(&MirrorProgram::Codex), "{kinds:?}");
    assert!(kinds.contains(&MirrorProgram::Pi), "{kinds:?}");
    let row = catalog.row(MirrorProgram::Claude, &claude).expect("the Claude row");
    assert_eq!(row.session.folder, "repo");
    assert_eq!(row.session.parent, "/", "the folder of the test is not under its home");
    assert_eq!(
        row.session.title, "Claude session",
        "a transcript without a last prompt names the program"
    );
    assert!(
        super::source_of(home.path(), MirrorProgram::Claude, &claude).is_some(),
        "the file of the entries of a session is found from its id"
    );
    let codex_row = catalog.row(MirrorProgram::Codex, codex).expect("the Codex row");
    assert_eq!(codex_row.session.title, format!("made-up title {codex}"));
    assert_eq!(
        catalog.unreadable(),
        [MirrorProgram::Antigravity],
        "a program whose store this build cannot read is named"
    );
}

#[test]
fn the_list_is_newest_first_and_holds_at_most_fifty_sessions() {
    let home = home();
    for index in 0..51 {
        let id = claude_id(index);
        claude_transcript(home.path(), &id, &[person_line(&format!("made-up session {index}"))]);
    }
    let catalog = read_list(home.path(), now(), &|_| false);

    assert_eq!(catalog.rows().len(), MAX_LIST_SESSIONS, "the list is cut at its bound");
    let times: Vec<i64> = catalog.rows().iter().map(|row| row.session.updated_at).collect();
    assert!(times.windows(2).all(|pair| pair[0] >= pair[1]), "{times:?}");
}

#[test]
fn a_session_older_than_the_bound_is_out_unless_a_process_runs_it() {
    let home = home();
    let old = claude_id(11);
    let path = claude_transcript(home.path(), &old, &[person_line("made-up request")]);
    let past = std::time::SystemTime::now() - std::time::Duration::from_hours(20 * 24);
    let held = std::fs::File::options()
        .write(true)
        .open(&path)
        .expect("the transcript");
    held.set_modified(past).expect("an old time");
    drop(held);

    let quiet = read_list(home.path(), now(), &|_| false);
    assert!(
        quiet.row(MirrorProgram::Claude, &old).is_none(),
        "a quiet session of twenty days is out"
    );

    claude_live(home.path(), 4_242, &old);
    let running = read_list(home.path(), now(), &|pid| pid == 4_242);
    let row = running.row(MirrorProgram::Claude, &old).expect("a session that runs");
    assert!(row.session.live, "the row says that a process runs it");
}

#[test]
fn no_tool_result_and_no_thinking_text_becomes_an_entry() {
    let home = home();
    let id = claude_id(21);
    let path = claude_transcript(
        home.path(),
        &id,
        &[
            person_line("made-up request"),
            serde_json::json!({
                "type": "assistant",
                "message": {"id": "m1", "role": "assistant", "content": [
                    {"type": "thinking", "thinking": "made-up thinking", "signature": "x"},
                    {"type": "text", "text": "made-up answer"},
                    {"type": "tool_use", "id": "t1", "name": "Bash",
                     "input": {"command": "made-up command\nsecond line"}},
                ]},
            })
            .to_string(),
            serde_json::json!({
                "type": "user",
                "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "made-up result"},
                ]},
            })
            .to_string(),
        ],
    );

    let entries = read_entries(MirrorProgram::Claude, &path);

    assert_eq!(
        entries,
        vec![
            MirrorEntry::Person {
                text: "made-up request".to_owned()
            },
            MirrorEntry::Agent {
                text: "made-up answer".to_owned()
            },
            MirrorEntry::Tool {
                name: "Bash".to_owned(),
                request: "made-up command".to_owned()
            },
        ]
    );
}

#[test]
fn the_entries_of_a_codex_and_of_a_pi_session_are_the_three_kinds() {
    let home = home();
    let codex = "0199aaaa-0000-7000-8000-000000000032";
    let rollout = codex_session(home.path(), codex, "/repo", now());
    write(
        &rollout,
        &format!(
            "{}\n{}\n{}\n{}\n{}\n",
            serde_json::json!({"type": "response_item", "payload": {"type": "message", "role": "user",
            "content": [
                {"type": "input_text", "text": "# AGENTS.md instructions for /repo\n\nmade-up rules"},
                {"type": "input_text", "text": "<environment_context>\n  <cwd>/repo</cwd>\n</environment_context>"},
                {"type": "input_text", "text": "made-up request"},
            ]}}),
            serde_json::json!({"type": "response_item", "payload": {"type": "reasoning",
                "content": [{"type": "reasoning_text", "text": "made-up thinking"}]}}),
            serde_json::json!({"type": "response_item", "payload": {"type": "message", "role": "assistant",
                "content": [{"type": "output_text", "text": "made-up answer"}]}}),
            serde_json::json!({"type": "response_item", "payload": {"type": "function_call",
                "name": "shell", "arguments": "{\"command\":\"made-up command\"}"}}),
            serde_json::json!({"type": "response_item", "payload": {"type": "custom_tool_call",
                "name": "exec", "input": "made-up script\nits second line"}}),
        ),
    );
    assert_eq!(
        read_entries(MirrorProgram::Codex, &rollout),
        vec![
            MirrorEntry::Notice {
                text: super::PROGRAM_TEXT_NOTICE.to_owned()
            },
            MirrorEntry::Person {
                text: "made-up request".to_owned()
            },
            MirrorEntry::Agent {
                text: "made-up answer".to_owned()
            },
            MirrorEntry::Tool {
                name: "shell".to_owned(),
                request: "made-up command".to_owned()
            },
            MirrorEntry::Tool {
                name: "exec".to_owned(),
                request: "made-up script".to_owned()
            },
        ],
        "the blocks that Codex writes before the words of the person are one notice"
    );

    let pi = "01a17ba5-0000-7000-8000-000000000033";
    let transcript = pi_session(home.path(), pi, "/repo");
    write(
        &transcript,
        &format!(
            "{}\n{}\n{}\n{}\n{}\n",
            serde_json::json!({"type": "session", "id": pi, "cwd": "/repo"}),
            serde_json::json!({"type": "message", "message": {"role": "user",
                "content": [{"type": "text", "text": "made-up request"}]}}),
            serde_json::json!({"type": "message", "message": {"role": "assistant",
                "content": [{"type": "thinking", "thinking": "made-up thinking"}]}}),
            serde_json::json!({"type": "message", "message": {"role": "assistant",
                "content": [{"type": "toolCall", "id": "t1", "name": "bash",
                    "arguments": {"command": "made-up command\nits second line", "timeout": 60}}]}}),
            serde_json::json!({"type": "message", "message": {"role": "assistant",
                "content": [{"type": "toolCall", "id": "t2", "name": "write",
                    "arguments": {"path": "made-up/file.txt", "content": "made-up content of a file"}}]}}),
        ),
    );
    assert_eq!(
        read_entries(MirrorProgram::Pi, &transcript),
        vec![
            MirrorEntry::Person {
                text: "made-up request".to_owned()
            },
            MirrorEntry::Tool {
                name: "bash".to_owned(),
                request: "made-up command".to_owned()
            },
            MirrorEntry::Tool {
                name: "write".to_owned(),
                request: "made-up/file.txt".to_owned()
            },
        ],
        "the request of a Pi tool is the first line of its one key, and the content of a file is no request"
    );
}

/// A real store of eleven thousand files ended the search for the file of a new
/// session before the search reached it, and the dock showed that session empty.
#[test]
fn the_file_of_a_new_codex_session_is_found_in_a_store_of_many_old_files() {
    let home = home();
    let old = home.path().join(".codex/sessions/2025/01/01");
    std::fs::create_dir_all(&old).expect("the old folder");
    for index in 0..4_100 {
        std::fs::write(old.join(format!("rollout-made-up-{index}.jsonl")), "").expect("an old file");
    }
    let id = "0199aaaa-0000-7000-8000-000000000077";
    let rollout = codex_session(home.path(), id, "/repo", now());
    assert_eq!(
        super::source_of(home.path(), MirrorProgram::Codex, id),
        Some(rollout),
        "the newest folders are searched first"
    );
}

/// A real run showed the text of an agent as one long line: its list and its
/// paragraphs were gone, and the marks of the list stood in the middle of a row.
#[test]
fn the_text_of_the_person_and_of_the_agent_keeps_its_lines() {
    let home = home();
    let id = claude_id(41);
    let path = claude_transcript(
        home.path(),
        &id,
        &[
            person_line("Do two things:\r\n  1. build\t\n  2. test\n\n\n\nThen stop."),
            agent_line("m1", "Done:\n- built\n- tested   \n"),
            agent_line("m1", "Nothing more waits.\u{7}"),
        ],
    );
    assert_eq!(
        read_entries(MirrorProgram::Claude, &path),
        vec![
            MirrorEntry::Person {
                text: "Do two things:\n  1. build\n  2. test\n\nThen stop.".to_owned()
            },
            MirrorEntry::Agent {
                text: "Done:\n- built\n- tested\n\nNothing more waits.".to_owned()
            },
        ],
        "the lines stay, a control character does not, and two parts of one message are two paragraphs"
    );
}

/// A real run showed a message of another session, with the words of the program
/// around it, as the words of the person.
#[test]
fn a_text_that_the_program_wrote_in_the_turn_of_the_person_is_one_notice() {
    let home = home();
    let id = claude_id(42);
    let meta = serde_json::json!({
        "type": "user", "isMeta": true,
        "message": {"role": "user", "content": "Caveat: the program wrote this line."},
    })
    .to_string();
    let path = claude_transcript(
        home.path(),
        &id,
        &[
            person_line("Start."),
            person_line("<system-reminder>\nA made-up reminder.\n</system-reminder>"),
            person_line("  <cross-session-message from=\"made-up\">hello</cross-session-message>"),
            person_line("<made-up-tag>\nA tag that no list names.\n</made-up-tag>"),
            person_line("<environment_context/>"),
            person_line("[Request interrupted by user]"),
            meta,
            person_line("<div> is what I typed, and a < b-c too."),
            person_line("<a-b"),
        ],
    );
    assert_eq!(
        read_entries(MirrorProgram::Claude, &path),
        vec![
            MirrorEntry::Person {
                text: "Start.".to_owned()
            },
            MirrorEntry::Notice {
                text: super::PROGRAM_TEXT_NOTICE.to_owned()
            },
            MirrorEntry::Person {
                text: "<div> is what I typed, and a < b-c too.".to_owned()
            },
            MirrorEntry::Person {
                text: "<a-b".to_owned()
            },
        ],
        "the texts of the program in a row are one notice, a line that the program marks is out, and a tag with no hyphen is the text of the person"
    );
}

#[test]
fn a_long_text_and_a_long_request_are_cut_at_their_bounds() {
    let home = home();
    let id = claude_id(41);
    let path = claude_transcript(
        home.path(),
        &id,
        &[serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": "x".repeat(MAX_TEXT_CHARS + 1)},
        })
        .to_string()],
    );

    let entries = read_entries(MirrorProgram::Claude, &path);

    let MirrorEntry::Person { text } = &entries[0] else {
        panic!("one text of the person: {entries:?}");
    };
    assert_eq!(text.chars().count(), MAX_TEXT_CHARS, "the text is cut at its bound");
    assert!(text.ends_with('…'), "the reader sees that the text is cut");
}

#[test]
fn a_line_that_does_not_parse_keeps_the_entries_around_it_and_one_notice() {
    let home = home();
    let id = claude_id(51);
    let path = claude_transcript(
        home.path(),
        &id,
        &[
            person_line("made-up request"),
            "{not json".to_owned(),
            agent_line("m1", "made-up answer"),
        ],
    );

    let entries = read_entries(MirrorProgram::Claude, &path);

    assert_eq!(entries.len(), 3, "{entries:?}");
    assert!(matches!(&entries[0], MirrorEntry::Person { .. }), "{entries:?}");
    assert!(matches!(&entries[1], MirrorEntry::Agent { .. }), "{entries:?}");
    assert_eq!(
        entries[2],
        MirrorEntry::Notice {
            text: UNREAD_NOTICE.to_owned()
        }
    );
}

#[test]
fn a_file_above_the_bound_reads_its_tail_and_says_so() {
    let home = home();
    let id = claude_id(61);
    let filler = format!("{}\n", agent_line("m0", &"y".repeat(2_000)));
    let mut lines = vec![filler; (usize::try_from(super::MAX_TAIL_BYTES).expect("a bound in bytes") / 2_000) + 4];
    lines.push(person_line("made-up last request"));
    let path = claude_transcript(home.path(), &id, &lines);

    let entries = read_entries(MirrorProgram::Claude, &path);

    assert!(
        std::fs::metadata(&path).expect("the file").len() > super::MAX_TAIL_BYTES,
        "the fixture is above the bound of one read"
    );
    assert_eq!(
        entries[0],
        MirrorEntry::Notice {
            text: super::CUT_NOTICE.to_owned()
        },
        "the reader says that it did not read the earlier part"
    );
    assert_eq!(
        entries.last(),
        Some(&MirrorEntry::Person {
            text: "made-up last request".to_owned()
        }),
        "the newest entry is in the tail"
    );
    assert!(
        entries.len() <= super::MAX_ENTRIES,
        "the entries are cut at their bound"
    );
}

#[test]
fn every_program_that_the_list_names_has_a_word_a_mark_and_a_name() {
    for program in PROGRAMS {
        assert!(!program.key().is_empty());
        assert_eq!(program.mark().chars().count(), 2, "{program:?}");
        assert!(!program.name().is_empty());
        assert_eq!(MirrorProgram::from_key(program.key()), Some(program));
    }
    assert_eq!(PROGRAMS.len(), 4);
    assert_eq!(MirrorProgram::from_key("another"), None);
}

#[test]
fn a_request_of_a_tool_is_one_line_of_at_most_two_hundred_characters() {
    let home = home();
    let id = claude_id(71);
    let path = claude_transcript(
        home.path(),
        &id,
        &[serde_json::json!({
            "type": "assistant",
            "message": {"id": "m1", "role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "Read",
                 "input": {"file_path": format!("{}\nand another line", "z".repeat(400))}},
            ]},
        })
        .to_string()],
    );

    let entries = read_entries(MirrorProgram::Claude, &path);

    let MirrorEntry::Tool { name, request } = &entries[0] else {
        panic!("one tool row: {entries:?}");
    };
    assert_eq!(name, "Read");
    assert_eq!(request.chars().count(), MAX_SUMMARY_CHARS);
    assert!(
        !request.contains('\n'),
        "only the first line of the request: {request:?}"
    );
}

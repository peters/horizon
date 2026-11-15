//! The text of the watch over a private `tmux` server: what it reports about the
//! panes of a machine, under each shell that must run it.

use std::path::PathBuf;
use std::time::Duration;

use crate::machine_watch::{MAX_PANES, REMOTE_TEXT, SLOW_INTERVAL_SECS, WATCH_FORMAT_VERSION, WATCH_INTERVAL_SECS};
use crate::util::stand_in_program_in;

use super::support::{self, Server, Shell, field, has, pane_of, run_text, run_text_with, wait_until};

/// The shell of a test that needs one shell and not every shell of this computer.
const SH: Shell = Shell {
    name: "sh",
    line: &["sh", "-c"],
};

#[test]
fn the_intervals_of_the_loop_are_the_two_named_consts() {
    assert!(
        REMOTE_TEXT.contains(&format!("FAST={WATCH_INTERVAL_SECS}")),
        "the loop waits the interval of the named const"
    );
    assert!(
        REMOTE_TEXT.contains(&format!("SLOW={SLOW_INTERVAL_SECS}")),
        "a machine with no tmux repeats that at the slower interval"
    );
    assert!(
        REMOTE_TEXT.contains(&format!("[ \"$i\" -le {MAX_PANES} ]")),
        "the text cuts its panes at the bound of the parser"
    );
    assert!(
        REMOTE_TEXT.contains("'HZ\\t%s\\n' \"$V\"") && REMOTE_TEXT.starts_with("V=1"),
        "the header of the text names the version of its format"
    );
}

#[test]
fn every_shell_of_this_computer_reports_the_panes_of_a_server() {
    let Some(server) = Server::new() else {
        return;
    };
    for shell in Shell::available() {
        let report = run_text(&server, &shell);
        assert_eq!(
            report.of_kind("HZ").next(),
            Some(format!("HZ\t{WATCH_FORMAT_VERSION}").as_str()),
            "{} names the version of the format first: {:?}",
            shell.name,
            report.lines
        );
        let pane = pane_of(&report, "warm-up");
        assert_eq!(
            field(pane, 2),
            "warm-up",
            "{} reports the session of its pane",
            shell.name
        );
        assert_eq!(
            report.lines.last().map(String::as_str),
            Some("Z"),
            "{} ends its report: {:?}",
            shell.name,
            report.lines
        );
    }
}

#[test]
fn a_machine_with_no_tmux_server_reports_that_in_one_line() {
    let report = run_text_with("hzw-no-such-server", &SH, |command| command);
    assert_eq!(report.single("X"), "X\tno-server");
    assert_eq!(
        report.of_kind("P").count(),
        0,
        "a machine with no server reports no pane"
    );
    assert_eq!(report.lines.last().map(String::as_str), Some("Z"));
}

#[test]
fn a_machine_with_no_tmux_program_reports_that_in_one_line() {
    let shell = Shell {
        name: "sh",
        line: &["/bin/sh", "-c"],
    };
    let report = run_text_with("hzw-no-program", &shell, |command| command.env("PATH", "/nonexistent"));
    assert_eq!(report.single("X"), "X\tno-tmux");
}

#[test]
fn a_pane_names_its_session_its_program_and_its_folder() {
    let Some(server) = Server::new() else {
        return;
    };
    let folder = std::env::temp_dir();
    server.session_in("hz-fields-1", &folder, "sleep 120");
    let report = run_text(&server, &SH);
    let pane = pane_of(&report, "hz-fields-1");
    assert!(field(pane, 1).starts_with('%'), "the pane id of the pane: {pane:?}");
    assert_eq!(field(pane, 3), "sleep", "the program of the pane");
    let named = folder.canonicalize().unwrap_or(folder);
    assert_eq!(
        field(pane, 4),
        named.to_str().expect("a folder of this computer"),
        "the folder of the pane, in the form that this system gives it"
    );
    assert_eq!(
        pane.split('\t').count(),
        6,
        "a pane line holds five values and no more: {pane:?}"
    );
}

#[test]
fn a_folder_and_a_session_with_a_tab_and_a_newline_stay_one_field() {
    let Some(server) = Server::new() else {
        return;
    };
    let folder = std::env::temp_dir().join("hz watch\tmany\nlines");
    std::fs::create_dir_all(&folder).expect("the folder of the test");
    server.session_in("hz flat", &folder, "sleep 120");
    let report = run_text(&server, &SH);
    let pane = pane_of(&report, "hz flat");
    assert_eq!(field(pane, 2), "hz flat", "the session is one field of words");
    let current = field(pane, 4);
    assert!(!current.contains('\t'), "no tab of a folder breaks a line: {current:?}");
    assert!(
        !current.contains('\n'),
        "no newline of a folder breaks a line: {current:?}"
    );
    // `tmux` 3.6 on Linux hands a control character of a path on, and another
    // version replaces it by `_`: both are one field of one line.
    let name = current.rsplit('/').next().unwrap_or_default();
    assert!(
        name.starts_with("hz watch") && name.contains("many") && name.ends_with("lines"),
        "the words of the folder stay in the field: {current:?}"
    );
}

#[test]
fn a_still_machine_sends_one_report_and_a_few_heartbeat_lines() {
    let Some(server) = Server::new() else {
        return;
    };
    let window = Duration::from_secs(7);
    let run = support::run_text_for(&server, &SH, window);
    assert_eq!(
        run.of_kind("Z").count(),
        1,
        "a machine whose report does not change sends one report in the window: {:?}",
        run.lines
    );
    assert!(
        run.of_kind("B").count() >= 2,
        "the loop sends one heartbeat line per interval: {:?}",
        run.lines
    );
    assert!(
        run.bytes() < 200,
        "a still machine sends a few bytes: {} bytes: {:?}",
        run.bytes(),
        run.lines
    );
}

#[test]
fn a_title_keeps_its_frame_without_a_utf8_locale() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some((_folder, titled)) = titled_pane() else {
        return;
    };
    server.session("hz-locale-1", &titled.display().to_string());
    wait_until("the frame of the title of the pane", || {
        server.title("hz-locale-1").starts_with('\u{280b}')
    });
    // An `ssh` command of a machine often has no UTF-8 locale, and the client of
    // `tmux` of such a shell prints `_` for every character outside ASCII. The text
    // of the watch asks with `-u`, so the frame of a Codex title arrives anyway.
    let report = run_text_with(server.socket(), &SH, |command| {
        command.env_remove("LANG").env_remove("LC_ALL").env_remove("LC_CTYPE")
    });
    let title = field(pane_of(&report, "hz-locale-1"), 5).to_owned();
    assert!(
        title.starts_with('\u{280b}'),
        "the title keeps its frame without a UTF-8 locale: {title:?}"
    );
    assert!(title.ends_with("working shop"), "the words of the title: {title:?}");
}

#[test]
fn a_codex_stand_in_is_a_pane_whose_title_names_its_state() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some((_folder, stand_in)) = codex_stand_in() else {
        return;
    };
    server.session("hz-codex-1", &stand_in.display().to_string());
    // The stand-in writes the titles of a turn when its person starts one, and it
    // reports the state of that turn until the turn ends.
    server.run(&["send-keys", "-t", "hz-codex-1", "start", "Enter"]);
    wait_until("a frame of the spinner in the title of the stand-in", || {
        server
            .title("hz-codex-1")
            .chars()
            .next()
            .is_some_and(|first| ('\u{2800}'..='\u{28ff}').contains(&first))
    });
    let report = run_text(&server, &SH);
    let pane = pane_of(&report, "hz-codex-1");
    assert_eq!(
        field(pane, 3),
        "python3",
        "the stand-in runs under an interpreter: {pane:?}"
    );
    let title = field(pane, 5);
    assert!(
        title
            .chars()
            .next()
            .is_some_and(|first| ('\u{2800}'..='\u{28ff}').contains(&first)),
        "the title of the pane is a frame of the spinner: {title:?}"
    );
    let pane_id = field(pane, 1);
    let command = report
        .of_kind("A")
        .find(|line| field(line, 1) == pane_id)
        .unwrap_or_else(|| panic!("the command line of the interpreter of the pane: {:?}", report.lines));
    assert!(
        command.contains("codex_attention_standin.py"),
        "the command line of the interpreter names the program: {command:?}"
    );
}

#[test]
fn a_working_line_that_a_pane_prints_is_one_of_its_last_rows() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some((_folder, script)) = working_line_script() else {
        return;
    };
    server.session("hz-work-1", &script.display().to_string());
    wait_until("a working line on the screen of the pane", || {
        server.screen("hz-work-1").lines().any(crate::is_agent_working_line)
    });
    let report = run_text(&server, &SH);
    let pane_id = field(pane_of(&report, "hz-work-1"), 1).to_owned();
    let rows: Vec<&str> = report.of_kind("R").filter(|line| field(line, 1) == pane_id).collect();
    assert!(
        !rows.is_empty(),
        "the pane reports the rows of its screen: {:?}",
        report.lines
    );
    assert!(
        rows.iter().any(|row| crate::is_agent_working_line(field(row, 2))),
        "a working line of the pane is one of its rows: {rows:?}"
    );
    assert!(rows.len() <= 6, "the rows of one pane stay in the bound: {rows:?}");
}

#[test]
fn a_pane_whose_program_is_no_interpreter_reports_no_command_line() {
    let Some(server) = Server::new() else {
        return;
    };
    server.session("hz-shell-1", "sleep 120");
    let report = run_text(&server, &SH);
    let pane_id = field(pane_of(&report, "hz-shell-1"), 1).to_owned();
    assert_eq!(
        report.of_kind("A").filter(|line| field(line, 1) == pane_id).count(),
        0,
        "a pane whose program is no interpreter reports no command line: {:?}",
        report.lines
    );
}

#[test]
fn seventy_panes_report_the_bound_and_no_more() {
    let Some(server) = Server::new() else {
        return;
    };
    // Seventy sessions of one pane each, in one call of the client: a case that
    // needs more panes than the bound cannot start one process per pane.
    let mut args: Vec<String> = Vec::new();
    for number in 1..=70 {
        if number > 1 {
            args.push(";".to_owned());
        }
        let session = format!("hz-many-{number}");
        args.extend([
            "new-session".to_owned(),
            "-d".to_owned(),
            "-s".to_owned(),
            session,
            "-x".to_owned(),
            "100".to_owned(),
            "-y".to_owned(),
            "20".to_owned(),
            "sleep".to_owned(),
            "120".to_owned(),
        ]);
    }
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    server.run(&borrowed);
    let report = run_text(&server, &SH);
    assert_eq!(
        report.of_kind("P").count(),
        MAX_PANES,
        "the text reports the panes of the bound and not the seventieth"
    );
}

/// The program of the Codex stand-in of this repository, under the name `codex`,
/// so a pane runs the stand-in as the interpreter program of the real one.
fn codex_stand_in() -> Option<(tempfile::TempDir, PathBuf)> {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/device-smoke/codex_attention_standin.py")
        .canonicalize()
        .ok()?;
    let python = ["python3", "python"].into_iter().find(|program| has(program))?;
    let folder = tempfile::tempdir().ok()?;
    let body = format!("#!/bin/sh\nexec {python} {} \"$@\"\n", script.display());
    let program = stand_in_program_in(folder.path(), "codex", &body).ok()?;
    Some((folder, program))
}

/// A program that writes the frame of a Codex turn into the title of its pane, and
/// keeps it there.
fn titled_pane() -> Option<(tempfile::TempDir, PathBuf)> {
    let folder = tempfile::tempdir().ok()?;
    let body = "#!/bin/sh\nprintf '\\033]0;\\342\\240\\213 working shop\\007'\nsleep 120\n";
    let program = stand_in_program_in(folder.path(), "titled", body).ok()?;
    Some((folder, program))
}

/// A program that keeps a working line at the bottom row of its pane.
fn working_line_script() -> Option<(tempfile::TempDir, PathBuf)> {
    let folder = tempfile::tempdir().ok()?;
    let body = "#!/bin/sh\nwhile :; do\n  printf '\\033[999;1H\\342\\240\\213 Working... (esc to interrupt)'\n  sleep 1\ndone\n";
    let program = stand_in_program_in(folder.path(), "working-line", body).ok()?;
    Some((folder, program))
}

//! The parser of a report of a machine: every bound, a cut line, another version of
//! the format and a line that is not UTF-8.

use crate::PanelKind;
use crate::machine_watch::{
    AgentPrograms, MAX_COMMAND_LINES, MAX_LINE_BYTES, MAX_PANES, MAX_REPORT_LINES, MAX_ROWS, MachineNote, ReportError,
    WATCH_FORMAT_VERSION, observations, parse_report,
};

/// One line of a report, as the text of the watch writes it.
fn line(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

/// One report of the lines, with its header and its terminator.
fn report_of(lines: &[&str]) -> Vec<Vec<u8>> {
    let mut all = vec![line(&format!("HZ\t{WATCH_FORMAT_VERSION}"))];
    all.extend(lines.iter().map(|text| line(text)));
    all.push(line("Z"));
    all
}

/// One pane line of the report of `pane`.
fn pane_line(pane: &str, session: &str, command: &str, cwd: &str, title: &str) -> String {
    format!("P\t{pane}\t{session}\t{command}\t{cwd}\t{title}")
}

#[test]
fn a_report_of_one_pane_is_one_pane_with_its_fields() {
    let lines = report_of(&[
        &pane_line("%3", "hz-1", "codex", "/home/person/shop", "⠙ shop"),
        "A\t%3\tnode /home/person/.local/bin/pi",
        "R\t%3\t⠋ Working... (esc to interrupt)",
        "R\t%3\t>",
    ]);
    let report = parse_report(&lines).expect("the report of the test parses");
    assert_eq!(report.note, None);
    assert_eq!(report.panes.len(), 1);
    let pane = report.panes.first().expect("the pane of the report");
    assert_eq!(pane.session, "hz-1");
    assert_eq!(pane.command, "codex");
    assert_eq!(pane.cwd, "/home/person/shop");
    assert_eq!(pane.title, "⠙ shop");
    assert_eq!(pane.command_lines, vec!["node /home/person/.local/bin/pi"]);
    assert_eq!(pane.rows, vec!["⠋ Working... (esc to interrupt)", ">"]);
}

#[test]
fn a_report_that_names_another_version_of_the_format_is_dropped() {
    let lines = vec![line("HZ\t2"), line("Z")];
    assert_eq!(
        parse_report(&lines),
        Err(ReportError::Version {
            version: "2".to_owned(),
            expected: WATCH_FORMAT_VERSION,
        })
    );
}

#[test]
fn a_report_without_its_header_or_its_terminator_is_dropped() {
    assert_eq!(
        parse_report(&[line("Z")]),
        Err(ReportError::Shape {
            line: 1,
            reason: "the report does not start with its header",
        })
    );
    assert_eq!(
        parse_report(&[line("HZ\t1"), line("P\t%0\thz\tcodex\t/w\tt")]),
        Err(ReportError::Shape {
            line: 2,
            reason: "the report does not end with its terminator",
        })
    );
    assert_eq!(
        parse_report(&[line("HZ\t1"), line("Z"), line("Z")]),
        Err(ReportError::Shape {
            line: 2,
            reason: "the line names no kind and no field",
        })
    );
    assert_eq!(
        parse_report(&[]),
        Err(ReportError::Shape {
            line: 1,
            reason: "the report is empty",
        })
    );
}

#[test]
fn a_line_that_is_not_utf8_drops_the_report() {
    let mut lines = vec![line("HZ\t1")];
    lines.push(vec![b'P', b'\t', b'%', b'0', b'\t', 0xff, 0xfe]);
    lines.push(line("Z"));
    assert_eq!(parse_report(&lines), Err(ReportError::NotUtf8 { line: 2 }));
}

#[test]
fn a_cut_line_and_a_line_of_another_shape_drop_the_report() {
    let cut = vec![b'P', b'\t', b'%', b'0', b'\t', b'h', b'z'];
    assert_eq!(
        parse_report(&[line("HZ\t1"), cut, line("Z")]),
        Err(ReportError::Shape {
            line: 2,
            reason: "a pane line holds five values",
        })
    );
    assert_eq!(
        parse_report(&report_of(&[&pane_line("%0", "hz", "codex", "/w", "t"), "Q\tx"])),
        Err(ReportError::Shape {
            line: 3,
            reason: "the line names no kind of the format",
        })
    );
    assert_eq!(
        parse_report(&report_of(&["R\t%9\tworking"])),
        Err(ReportError::Shape {
            line: 2,
            reason: "a row names a pane that is no pane of the report",
        })
    );
    assert_eq!(
        parse_report(&report_of(&[&pane_line("0", "hz", "codex", "/w", "t")])),
        Err(ReportError::Shape {
            line: 2,
            reason: "a pane id is '%' and digits",
        })
    );
    assert_eq!(
        parse_report(&report_of(&["X\tsomething"])),
        Err(ReportError::Shape {
            line: 2,
            reason: "a note is 'no-tmux' or 'no-server'",
        })
    );
}

#[test]
fn a_note_of_the_machine_is_one_line_of_the_report() {
    let note = parse_report(&report_of(&["X\tno-server"])).expect("the note parses");
    assert_eq!(note.note, Some(MachineNote::NoServer));
    assert!(note.panes.is_empty());
    let note = parse_report(&report_of(&["X\tno-tmux"])).expect("the note parses");
    assert_eq!(note.note, Some(MachineNote::NoTmux));
}

#[test]
fn the_bounds_of_a_report_drop_it_whole() {
    let too_long = vec![line("HZ\t1"), vec![b'P'; MAX_LINE_BYTES + 1], line("Z")];
    assert_eq!(
        parse_report(&too_long),
        Err(ReportError::Bound {
            what: "one line in bytes",
            limit: MAX_LINE_BYTES
        })
    );

    let many_lines: Vec<Vec<u8>> = std::iter::once(line("HZ\t1"))
        .chain((0..MAX_REPORT_LINES).map(|_| line("zp")))
        .chain(std::iter::once(line("Z")))
        .collect();
    assert_eq!(
        parse_report(&many_lines),
        Err(ReportError::Bound {
            what: "the count of lines",
            limit: MAX_REPORT_LINES,
        })
    );

    let mut many_panes = vec![line("HZ\t1")];
    for number in 0..=MAX_PANES {
        many_panes.push(line(&pane_line(&format!("%{number}"), "hz", "codex", "/w", "t")));
    }
    many_panes.push(line("Z"));
    assert_eq!(
        parse_report(&many_panes),
        Err(ReportError::Bound {
            what: "the count of panes",
            limit: MAX_PANES
        })
    );

    let mut many_rows = vec![line("HZ\t1"), line(&pane_line("%0", "hz", "codex", "/w", "t"))];
    for _ in 0..=MAX_ROWS {
        many_rows.push(line("R\t%0\tworking"));
    }
    many_rows.push(line("Z"));
    assert_eq!(
        parse_report(&many_rows),
        Err(ReportError::Bound {
            what: "the rows of one pane",
            limit: MAX_ROWS
        })
    );

    let mut many_commands = vec![line("HZ\t1"), line(&pane_line("%0", "hz", "node", "/w", "t"))];
    for _ in 0..=MAX_COMMAND_LINES {
        many_commands.push(line("A\t%0\tnode /home/person/.local/bin/pi"));
    }
    many_commands.push(line("Z"));
    assert_eq!(
        parse_report(&many_commands),
        Err(ReportError::Bound {
            what: "the command lines of one pane",
            limit: MAX_COMMAND_LINES,
        })
    );
}

#[test]
fn the_programs_of_the_agent_kinds_name_their_kind() {
    let programs = AgentPrograms::builtin();
    assert_eq!(programs.kind_of("codex"), Some(PanelKind::Codex));
    assert_eq!(programs.kind_of("claude"), Some(PanelKind::Claude));
    assert_eq!(programs.kind_of("pi"), Some(PanelKind::Pi));
    assert_eq!(programs.kind_of("node"), None, "an interpreter names no agent");
    assert_eq!(programs.kind_of("bash"), None);
}

#[test]
fn a_pane_from_a_program_of_an_agent_kind_is_one_agent_with_its_folder() {
    let programs = AgentPrograms::builtin();
    let report = parse_report(&report_of(&[
        &pane_line("%0", "hz-1", "codex", "/home/person/shop", "⠙ shop"),
        &pane_line("%1", "hz-1", "bash", "/home/person/shop", "shop"),
    ]))
    .expect("the report parses");
    let agents = observations(&report, &programs);
    assert_eq!(agents.len(), 1, "the shell pane is no agent");
    let agent = &agents[0];
    assert_eq!(agent.kind, PanelKind::Codex);
    assert_eq!(agent.session, "hz-1");
    assert_eq!(agent.pane, "%0");
    assert_eq!(agent.name, "shop", "the name is the last part of the folder");
    assert_eq!(agent.title, crate::agent_work::TitleState::Working);
    assert!(
        !agent.working,
        "the Codex title answers and no row holds a working line"
    );
}

#[test]
fn a_pane_of_an_interpreter_is_the_agent_of_the_program_that_it_runs() {
    let programs = AgentPrograms::builtin();
    let report = parse_report(&report_of(&[
        &pane_line("%4", "hz-1", "node", "/home/person/work", "work"),
        "A\t%4\tsleep 30",
        "A\t%4\tnode /home/person/.local/bin/pi --resume abc",
        "R\t%4\t⠋ Working... (esc to interrupt)",
    ]))
    .expect("the report parses");
    let agents = observations(&report, &programs);
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].kind, PanelKind::Pi);
    assert!(agents[0].working, "the row of the pane holds a working line");
    assert_eq!(agents[0].title, crate::agent_work::TitleState::Plain);
}

#[test]
fn the_name_of_an_agent_is_the_last_part_of_its_folder_cut_at_the_bound() {
    let programs = AgentPrograms::builtin();
    let long = "x".repeat(crate::machine_watch::MAX_NAME_CHARS + 20);
    let report = parse_report(&report_of(&[
        &pane_line("%0", "hz-1", "codex", &format!("/home/person/{long}"), "title"),
        &pane_line("%1", "hz-1", "codex", "/", "title"),
    ]))
    .expect("the report parses");
    let agents = observations(&report, &programs);
    assert_eq!(agents[0].name.chars().count(), crate::machine_watch::MAX_NAME_CHARS);
    assert_eq!(
        agents[1].name, "Codex",
        "a folder that names no part gives the name of the kind"
    );
}

#[test]
fn a_preset_of_an_agent_kind_names_its_kind_too() {
    let preset = crate::config::PresetConfig {
        name: "Shop agent".to_owned(),
        alias: None,
        kind: PanelKind::Claude,
        command: Some("/usr/local/bin/shop-agent --fast".to_owned()),
        args: Vec::new(),
        resume: crate::panel::PanelResume::Fresh,
        ssh_connection: None,
    };
    let programs = AgentPrograms::builtin().with_presets(&[preset]);
    assert_eq!(programs.kind_of("shop-agent"), Some(PanelKind::Claude));
}

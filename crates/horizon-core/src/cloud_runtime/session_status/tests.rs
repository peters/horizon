use super::*;

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn command_carries_the_script_and_only_valid_ids() {
    let command = command(&ids(&["agent-1", "shell_2"])).unwrap();
    let encoded = command
        .strip_prefix("printf %s '")
        .and_then(|rest| rest.split_once('\'').map(|(encoded, _)| encoded))
        .unwrap();
    let decoded = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap();
    assert_eq!(decoded, SCRIPT.as_bytes());
    assert!(command.ends_with("| base64 -d | python3 - agent-1 shell_2"));
    for refused in [vec![], ids(&["ok", "a;rm -rf /"]), ids(&["$(id)"]), ids(&[""])] {
        assert!(super::command(&refused).is_err(), "{refused:?}");
    }
    let too_many: Vec<String> = (0..=MAX_SESSIONS).map(|n| format!("s{n}")).collect();
    assert!(super::command(&too_many).is_err());
}

#[test]
fn parse_classifies_and_bounds_each_session() {
    let output = serde_json::json!({"sessions": [
        {"id": "busy", "state": "running", "activity_age_seconds": 3,
         "lines": ["building", "✻ Working… (12s · esc to interrupt)"]},
        {"id": "quiet", "state": "running", "activity_age_seconds": 400,
         "lines": ["done.", "\u{1b}[31m> \u{7}", ""]},
        {"id": "ended", "state": "exited", "exit_status": 2, "lines": ["boom"]},
        {"id": "gone", "state": "missing"},
        {"id": "stranger", "state": "running", "lines": ["not asked for"]},
        {"id": "long", "state": "running",
         "lines": (0..20).map(|n| format!("{n}{}", "x".repeat(500))).collect::<Vec<_>>()}
    ]})
    .to_string();
    let status = parse(&output, &ids(&["busy", "quiet", "ended", "gone", "long"])).unwrap();
    let by_id = |id: &str| status.iter().find(|s| s.id == id).unwrap();
    assert_eq!(status.len(), 5);
    assert_eq!(by_id("busy").activity, SessionActivity::Working);
    assert_eq!(by_id("busy").quiet_for, Some(Duration::from_secs(3)));
    assert_eq!(by_id("quiet").activity, SessionActivity::Idle);
    assert_eq!(by_id("quiet").last_line(), Some("[31m>"));
    assert_eq!(by_id("ended").activity, SessionActivity::Exited(Some(2)));
    assert_eq!(by_id("gone").activity, SessionActivity::Missing);
    assert!(by_id("gone").lines.is_empty());
    let long = by_id("long");
    assert_eq!(long.lines.len(), MAX_LINES);
    assert!(long.lines[0].starts_with("12"));
    assert!(long.lines.iter().all(|line| line.chars().count() <= MAX_LINE_CHARS));
    assert!(parse("not json", &ids(&["busy"])).is_err());
}

#[cfg(unix)]
#[test]
fn script_reads_sessions_from_tmux_without_changing_them() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("tmux.log");
    let tmux = temp.path().join("tmux");
    std::fs::write(
        &tmux,
        format!(
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' \"$*\" >> '{log}'\n",
                "case \"$*\" in\n",
                "  *'=gone:'*) exit 1;;\n",
                "  *display-message*) echo 1000;;\n",
                "  *capture-pane*) printf 'first\\n\\n  \\nlast line\\n';;\n",
                "esac\n"
            ),
            log = log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&tmux, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        temp.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = std::process::Command::new("python3")
        .arg("-")
        .args(["live", "gone"])
        .env("PATH", path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(SCRIPT.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
    let status = parse(&String::from_utf8(output.stdout).unwrap(), &ids(&["live", "gone"])).unwrap();
    assert_eq!(status[0].activity, SessionActivity::Idle);
    assert_eq!(status[0].lines, ["first", "last line"]);
    assert!(status[0].quiet_for.is_some());
    assert_eq!(status[1].activity, SessionActivity::Missing);
    let calls = std::fs::read_to_string(log).unwrap();
    assert!(
        calls
            .lines()
            .all(|call| call.starts_with("-L horizon-cloud display-message")
                || call.starts_with("-L horizon-cloud capture-pane")),
        "{calls}"
    );
}

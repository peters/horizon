use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::agent_work::{ResumePolicy, StartupPlan, WorkLaunch};
use crate::runtime_state::AgentSessionBinding;
use crate::{PanelId, PanelOptions, WorkspaceId};

#[test]
fn confirmed_restart_replaces_the_process_and_submits_the_brief_once() {
    const CHILD_ENV: &str = "HORIZON_TEST_MANUAL_RESTART_CHILD";
    if std::env::var_os(CHILD_ENV).is_some() {
        exercise_confirmed_restart();
        return;
    }
    let home = tempfile::tempdir().expect("isolated home");
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).expect("fixture bin");
    std::fs::write(home.path().join(".bashrc"), "").expect("isolated shell config");
    let provider = bin.join("pi");
    std::fs::write(
        &provider,
        r#"#!/bin/bash
if test -f "$HOME/provider.pid"; then
    read -r previous < "$HOME/provider.pid"
    if kill -0 "$previous" 2>/dev/null; then
        printf 'overlapping processes\n' > "$HOME/overlap"
    fi
fi
printf '%s\0' "$@" > "$HOME/args.$$"
printf '%s\n' "$$" > "$HOME/provider.pid"
trap 'rm -f "$HOME/.claude/sessions/$$.json"; exit 0' HUP TERM
printf '%s\n' "$$" >> "$HOME/launches"
while :; do read -r -t 1 || :; done
"#,
    )
    .expect("fixture executable");
    std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700)).expect("executable permission");
    std::fs::copy(&provider, bin.join("claude")).expect("second disposable provider");
    // Re-exec isolates HOME, PATH and SHELL from concurrent tests and ensures
    // the ordinary launch resolver can only find our disposable provider.
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            concat!(
                module_path!(),
                "::confirmed_restart_replaces_the_process_and_submits_the_brief_once"
            )
            .trim_start_matches("horizon_core::"),
            "--nocapture",
        ])
        .env_clear()
        .env(CHILD_ENV, "1")
        .env("HOME", home.path())
        .env("SHELL", "/bin/bash")
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .current_dir(home.path())
        .output()
        .expect("isolated test process");
    assert!(
        output.status.success(),
        "isolated restart failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let launches = std::fs::read_to_string(home.path().join("launches")).expect("test child executed");
    assert_eq!(launches.lines().count(), 5);
    assert!(
        !home.path().join("overlap").exists(),
        "replacement started before previous process exited"
    );
}

fn exercise_confirmed_restart() {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").expect("fixture home"));
    let saved_session = write_pi_session(&home);
    let mut panel = restored_provider(&home, PanelKind::Pi);
    assert_eq!(wait_for_launch(&home, 1), ["--session", "fixture-session"]);
    panel.check_saved_work_session().expect("catalog-backed session");
    exercise_pi_history(&mut panel, &home, &saved_session);
    exercise_catalog_rows(&mut panel, &home);
    assert_refusal_preserves_process(&mut panel, &home, "saved conversation", || {
        std::fs::remove_file(saved_session).expect("delete saved session after confirmation");
    });
    let _ = write_pi_session(&home);
    assert_refusal_preserves_process(&mut panel, &home, "owned executable", || {
        std::fs::write(home.join(".bashrc"), "pi() { :; }").expect("replace executable resolution");
    });
    std::fs::write(home.join(".bashrc"), "").expect("restore executable resolution");
    panel.request_work_resume().expect("confirm offered continuation");
    panel.restart().expect("confirmed restart");
    let args = wait_for_launch(&home, 2);
    assert_eq!(args.len(), 3, "exactly one brief must be submitted");
    assert_eq!(&args[..2], ["--session", "fixture-session"]);
    assert!(args[2].starts_with("The user selected Resume work in Horizon for this conversation."));
    assert!(panel.terminal().expect("terminal").pending_work_resume().is_none());
    assert!(panel.requested_work_brief().expect("consumed authorization").is_none());
    panel.restart().expect("ordinary restart after continuation");
    assert_eq!(wait_for_launch(&home, 3), ["--session", "fixture-session"]);
    shutdown_for_restart(panel.terminal_mut().expect("terminal"), true).expect("close disposable provider");

    let mut panel = restored_provider(&home, PanelKind::Claude);
    let _ = wait_for_launch(&home, 4);
    let project = home.join(".claude/projects/fixture");
    std::fs::create_dir_all(&project).expect("transcript directory");
    let transcript = project.join("fixture-session.jsonl");
    let valid = concat!(
        "{\"type\":\"mode\",\"sessionId\":\"fixture-session\",\"mode\":\"normal\"}\n",
        "{\"type\":\"file-history-snapshot\",\"snapshot\":{}}\n",
        "{\"type\":\"user\",\"sessionId\":\"fixture-session\",\"message\":{\"role\":\"user\",\"content\":\"Fixture request\"}}\n"
    );
    std::fs::write(&transcript, valid).expect("saved transcript");
    panel.check_saved_work_session().expect("metadata-prefixed history");
    assert!(crate::runtime_state::claude_session_transcript_exists(
        "fixture-session"
    ));
    assert_refusal_preserves_process(&mut panel, &home, "saved conversation", || {
        std::fs::remove_file(&transcript).expect("remove transcript after confirmation");
    });
    for invalid in [
        String::new(),
        "{invalid}\n".into(),
        "{}\n".into(),
        valid.replace("fixture-session", "other-session"),
    ] {
        std::fs::write(&transcript, valid).expect("restore valid history");
        assert_refusal_preserves_process(&mut panel, &home, "saved conversation", || {
            std::fs::write(&transcript, invalid).expect("invalidate history after confirmation");
        });
    }
    std::fs::write(&transcript, valid).expect("repair history");
    exercise_competing_session(&mut panel, &home);
    panel.request_work_resume().expect("confirm repaired history");
    panel.restart().expect("resume repaired history");
    let args = wait_for_launch(&home, 5);
    assert_eq!(&args[..2], ["--resume", "fixture-session"]);
    assert_eq!(
        args.iter()
            .filter(|arg| arg.starts_with("The user selected Resume work"))
            .count(),
        1
    );
    shutdown_for_restart(panel.terminal_mut().expect("terminal"), true).expect("close repaired-history provider");
}

fn write_pi_session(home: &std::path::Path) -> std::path::PathBuf {
    let directory = home.join(".pi/agent/sessions/fixture");
    std::fs::create_dir_all(&directory).expect("session directory");
    let path = directory.join("2026-01-01T00-00-00_fixture-session.jsonl");
    let session = serde_json::json!({
        "type": "session", "id": "fixture-session", "cwd": home,
        "timestamp": "2026-01-01T00:00:00.000Z", "version": 3
    });
    std::fs::write(&path, format!("{session}\n")).expect("saved session");
    path
}

fn exercise_pi_history(panel: &mut Panel, home: &std::path::Path, path: &std::path::Path) {
    let valid = std::fs::read_to_string(path).expect("saved header");
    std::fs::File::open(path)
        .expect("session file")
        .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)))
        .expect("old session timestamp");
    for index in 0..129 {
        std::fs::write(
            path.with_file_name(format!("newer-{index}.jsonl")),
            valid.replace("fixture-session", &format!("newer-{index}")),
        )
        .expect("newer session");
    }
    panel
        .check_saved_work_session()
        .expect("exact old session beyond picker limit");
    for hidden in [
        path.parent()
            .expect("cwd directory")
            .join("nested")
            .join(path.file_name().expect("session name")),
        home.join(".pi/agent/sessions")
            .join(path.file_name().expect("session name")),
    ] {
        std::fs::create_dir_all(hidden.parent().expect("hidden directory")).expect("hidden directory");
        assert_refusal_preserves_process(panel, home, "saved conversation", || {
            std::fs::rename(path, &hidden).expect("move outside native discovery layout");
        });
        std::fs::rename(&hidden, path).expect("restore discoverable session");
    }
    for invalid in [
        String::new(),
        "{invalid}\n".into(),
        "{}\n".into(),
        valid.replace("fixture-session", "wrong-session"),
        valid.replace("\"version\":3", "\"parentSession\":\"parent\",\"version\":3"),
    ] {
        assert_refusal_preserves_process(panel, home, "saved conversation", || {
            std::fs::write(path, invalid).expect("invalidate saved session");
        });
        std::fs::write(path, &valid).expect("restore valid header");
    }
    std::fs::write(path, format!("{{invalid}}\n\n{valid}")).expect("native-tolerated preamble");
    panel.check_saved_work_session().expect("native header scan");
    std::fs::write(path, valid).expect("restore header");
}

fn exercise_competing_session(panel: &mut Panel, home: &std::path::Path) {
    let directory = home.join(".claude/sessions");
    std::fs::create_dir_all(&directory).expect("registry");
    let pid = panel
        .terminal()
        .expect("terminal")
        .owned_process_id()
        .expect("owned live process");
    std::fs::write(
        directory.join(format!("{pid}.json")),
        serde_json::json!({"sessionId":"fixture-session", "pid":pid}).to_string(),
    )
    .expect("own registry entry");
    panel
        .check_work_session_exit(Some(pid))
        .expect("exclude verified owned process only");
    let competitor = directory.join("competing.json");
    assert_refusal_preserves_process(panel, home, "another process", || {
        std::fs::write(
            &competitor,
            serde_json::json!({"sessionId":"fixture-session", "pid":std::process::id()}).to_string(),
        )
        .expect("competing registry entry");
    });
    std::fs::remove_file(competitor).expect("remove competing entry");
}

fn exercise_indexed_history(
    panel: &mut Panel,
    home: &std::path::Path,
    connection: &rusqlite::Connection,
    path: &std::path::Path,
    header: &serde_json::Value,
) {
    if panel.kind == PanelKind::Codex {
        let preamble = "{\"type\":\"event_msg\",\"payload\":{}}\n".repeat(12);
        std::fs::write(path, format!("{preamble}{header}\n")).expect("metadata preamble");
        panel
            .check_saved_work_session()
            .expect("metadata after more than eight rows");
        assert_refusal_preserves_process(panel, home, "saved conversation", || {
            std::fs::write(
                path,
                format!("{preamble}{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"wrong-session\"}}}}\n"),
            )
            .expect("wrong metadata identity");
        });
    } else {
        connection
            .execute_batch(
                "WITH RECURSIVE newer(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM newer WHERE n<1001)
            INSERT INTO session_docs SELECT 'newer-' || n, '/repo', 2, 'newer' FROM newer;",
            )
            .expect("newer index rows");
        panel
            .check_saved_work_session()
            .expect("exact old session beyond picker limit");
        for invalid in [
            "{invalid}",
            "{\"type\":\"invalid\",\"content\":null}",
            "{\"type\":\"user\",\"content\":null}",
            "{\"type\":\"system\",\"content\":[]}",
            "{\"type\":\"user\",\"content\":[{\"type\":\"text\",\"text\":false}]}",
        ] {
            std::fs::write(path, format!("{header}\n")).expect("repair history");
            assert_refusal_preserves_process(panel, home, "saved conversation", || {
                std::fs::write(path, invalid).expect("malformed history");
            });
        }
        for valid in [
            serde_json::json!({"type":"system","content":"fixture"}),
            serde_json::json!({"type":"user","content":[{"type":"text","text":"fixture"}]}),
        ] {
            std::fs::write(path, format!("{valid}\n")).expect("native message shape");
            panel.check_saved_work_session().expect("native message shape");
        }
    }
    std::fs::write(path, format!("{header}\n")).expect("restore indexed history");
}

fn exercise_catalog_rows(panel: &mut Panel, home: &std::path::Path) {
    for (kind, path, table, schema) in [
            (
                PanelKind::Codex,
                home.join(".codex/state_5.sqlite"),
                "threads",
                "CREATE TABLE threads (id TEXT, rollout_path TEXT, source TEXT, title TEXT, cwd TEXT, updated_at INTEGER, archived INTEGER);
                 INSERT INTO threads VALUES ('fixture-session', NULL, 'cli', 'Fixture', '/repo', 1, 0);",
            ),
            (
                PanelKind::OpenCode,
                crate::opencode_paths::opencode_db_path().expect("session DB path"),
                "session",
                "CREATE TABLE session (id TEXT, title TEXT, directory TEXT, time_updated INTEGER, time_archived INTEGER, parent_id TEXT);
                 INSERT INTO session VALUES ('fixture-session', 'Fixture', '/repo', 1, NULL, NULL);",
            ),
            (
                PanelKind::Grok,
                crate::local_store::grok_sessions_db_path().expect("session DB path"),
                "session_docs",
                "CREATE TABLE session_docs (session_id TEXT, cwd TEXT, updated_at INTEGER, title TEXT);
                 INSERT INTO session_docs VALUES ('fixture-session', '/repo', 1, 'Fixture');",
            ),
        ] {
            panel.kind = kind;
            panel.session_binding.as_mut().expect("binding").kind = kind;
            // The matching Pi ID must not satisfy another provider's lookup.
            assert!(panel.check_saved_work_session().is_err(), "{kind:?} missing store");
            std::fs::create_dir_all(path.parent().expect("DB directory")).expect("DB directory");
            let connection = rusqlite::Connection::open(&path).expect("fixture DB");
            connection.execute_batch(schema).expect("saved session row");
            let backing = match kind {
                PanelKind::Codex => {
                    assert!(panel.check_saved_work_session().is_err(), "NULL rollout path");
                    let path = home.join(".codex/sessions/fixture.jsonl");
                    connection.execute("UPDATE threads SET rollout_path = ?1", [path.to_str().expect("path")]).expect("rollout path");
                    Some((path, serde_json::json!({"type": "session_meta", "payload": {"id": "fixture-session"}})))
                }
                PanelKind::Grok => Some((
                    home.join(".grok/sessions/%2Frepo/fixture-session/chat_history.jsonl"),
                    serde_json::json!({"type": "user", "content": "Fixture request"}),
                )),
                _ => None,
            };
            if let Some((path, header)) = &backing {
                assert!(panel.check_saved_work_session().is_err(), "{kind:?} stale index without backing file");
                std::fs::create_dir_all(path.parent().expect("session directory")).expect("session directory");
                std::fs::write(path, format!("{header}\n")).expect("saved transcript");
                panel.check_saved_work_session().expect("indexed readable transcript");
                exercise_indexed_history(panel, home, &connection, path, header);
                assert_refusal_preserves_process(panel, home, "saved conversation", || {
                    std::fs::remove_file(path).expect("remove transcript while retaining index row");
                });
                assert!(panel.check_saved_work_session().is_err(), "{kind:?} deleted backing file");
                std::fs::write(path, "").expect("empty saved transcript");
                assert!(panel.check_saved_work_session().is_err(), "{kind:?} empty backing file");
                std::fs::write(path, format!("{header}\n")).expect("restore saved transcript");
            }
            panel.check_saved_work_session().expect("provider-scoped saved session");
            connection.execute(&format!("DELETE FROM {table}"), []).expect("delete session");
            assert!(panel.check_saved_work_session().is_err(), "{kind:?} deleted session");
        }
    panel.kind = PanelKind::Pi;
    panel.session_binding.as_mut().expect("binding").kind = PanelKind::Pi;
}

fn assert_refusal_preserves_process(
    panel: &mut Panel,
    home: &std::path::Path,
    reason: &str,
    invalidate: impl FnOnce(),
) {
    let original_pid = std::fs::read_to_string(home.join("provider.pid")).expect("original PID");
    let launches = std::fs::read(home.join("launches")).expect("original launches");
    panel
        .request_work_resume()
        .expect("confirm before target becomes unavailable");
    invalidate();
    let error = panel.restart().expect_err("unavailable continuation must be refused");
    assert!(error.to_string().contains(reason), "unexpected refusal: {error}");
    assert!(
        std::process::Command::new("/bin/bash")
            .args(["-c", "kill -0 \"$1\"", "fixture-probe", original_pid.trim()])
            .status()
            .expect("probe original process")
            .success(),
        "refused continuation shut down the original process"
    );
    assert_eq!(std::fs::read(home.join("launches")).expect("launches"), launches);
    assert!(panel.terminal().expect("terminal").pending_work_resume().is_some());
    assert!(
        panel
            .requested_work_brief()
            .expect("refused authorization consumed")
            .is_none()
    );
}

fn restored_provider(home: &std::path::Path, kind: PanelKind) -> Panel {
    Panel::spawn(
        PanelId(1),
        WorkspaceId(1),
        PanelOptions {
            kind,
            cwd: Some(home.to_path_buf()),
            local_id: Some("manual-restart-fixture".into()),
            is_restore: true,
            work_resume: ResumePolicy {
                enabled: true,
                ..Default::default()
            },
            session_binding: Some(AgentSessionBinding::new(
                kind,
                "fixture-session".into(),
                None,
                None,
                None,
            )),
            ..Default::default()
        },
    )
    .expect("restored disposable provider")
}

fn wait_for_launch(home: &std::path::Path, expected: usize) -> Vec<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let launches = std::fs::read_to_string(home.join("launches")).unwrap_or_default();
        let pids: Vec<_> = launches.lines().collect();
        assert!(pids.len() <= expected, "unexpected additional provider launch");
        if pids.len() == expected && launches.ends_with('\n') {
            let bytes = std::fs::read(home.join(format!("args.{}", pids[expected - 1]))).expect("provider args");
            return bytes
                .strip_suffix(&[0])
                .expect("NUL terminated arguments")
                .split(|byte| *byte == 0)
                .map(|arg| String::from_utf8(arg.to_vec()).expect("UTF-8 argument"))
                .collect();
        }
        assert!(std::time::Instant::now() < deadline, "provider did not launch");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn fixture() -> Panel {
    let mut panel = Panel::spawn(
        PanelId(1),
        WorkspaceId(1),
        PanelOptions {
            command: Some("/bin/sh".into()),
            args: vec!["-c".into(), "cat >/dev/null".into()],
            ..Default::default()
        },
    )
    .expect("disposable shell");
    panel.kind = PanelKind::Pi;
    panel.launch_command = None;
    panel.launch_args.clear();
    panel.session_binding = Some(AgentSessionBinding {
        kind: PanelKind::Pi,
        session_id: "fixture-session".into(),
        cwd: None,
        label: None,
        updated_at: None,
    });
    panel.work_resume = ResumePolicy {
        enabled: true,
        ..Default::default()
    };
    let plan = StartupPlan::prepare(
        &WorkLaunch {
            panel: &panel.local_id,
            kind: panel.kind,
            policy: &panel.work_resume,
            cwd: None,
            session_id: Some("fixture-session"),
            default_command: true,
        },
        true,
        true,
    );
    panel.terminal_mut().expect("terminal").work_continuation = plan.state;
    panel.terminal_mut().expect("terminal").work_continuation.owns_process = true;
    panel
}
#[test]
fn manual_request_is_revoked_by_user_input_or_disabling_policy() {
    let mut panel = fixture();
    assert!(panel.request_work_resume().is_ok());
    assert!(panel.requested_work_brief().expect("brief").is_some());
    panel.request_work_resume().expect("request before cancellation");
    panel.terminal().expect("terminal").write_input(b"new request\n");
    assert!(panel.requested_work_brief().is_err());
    assert!(panel.requested_work_brief().expect("consumed refusal").is_none());
    panel.set_work_resume_enabled(false);
    assert!(panel.terminal().expect("terminal").pending_work_resume().is_none());
    assert!(panel.request_work_resume().is_err());
    shutdown_for_restart(panel.terminal_mut().expect("terminal"), true).expect("close fixture");
}
#[test]
fn custom_arguments_and_missing_bindings_cannot_target_a_different_conversation() {
    let mut panel = fixture();
    panel.launch_args = vec!["--session".into(), "different".into()];
    assert!(panel.request_work_resume().is_err());
    panel.launch_args.clear();
    panel.session_binding = None;
    assert!(panel.request_work_resume().is_err());
    shutdown_for_restart(panel.terminal_mut().expect("terminal"), true).expect("close fixture");
}
#[test]
fn changing_the_binding_revokes_the_offer_and_consumes_queued_authorization() {
    let mut panel = fixture();
    panel.request_work_resume().expect("request");
    panel.session_binding.as_mut().expect("binding").session_id = "different-session".into();
    assert!(panel.check_work_resume().is_err());
    assert!(panel.request_work_resume().is_err());
    assert!(panel.requested_work_brief().is_err());
    assert!(panel.requested_work_brief().expect("refusal consumed").is_none());
    shutdown_for_restart(panel.terminal_mut().expect("terminal"), true).expect("close fixture");
}

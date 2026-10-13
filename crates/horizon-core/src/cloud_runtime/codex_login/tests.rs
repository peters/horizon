use super::*;
use base64::Engine as _;

fn id_token(email: &str, plan: &str) -> String {
    let encode = |value: serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap())
    };
    format!(
        "{}.{}.synthetic-signature",
        encode(serde_json::json!({"alg": "none"})),
        encode(serde_json::json!({
            "email": email,
            "https://api.openai.com/auth": {"chatgpt_plan_type": plan},
        })),
    )
}

fn login(account: &str, refreshed: Option<&str>, token: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": id_token("person@example.test", "plus"),
            "access_token": token,
            "refresh_token": format!("{token}-refresh"),
            "account_id": account,
        },
        "last_refresh": refreshed,
    }))
    .unwrap()
}

fn private_home() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    private_directory(directory.path()).unwrap();
    directory
}

fn stored(home: &Path, bytes: &[u8]) {
    write_private(home, bytes).unwrap();
}

#[test]
fn only_a_complete_chatgpt_login_counts() {
    let complete = login(
        "synthetic-account",
        Some("2026-10-01T08:00:00.123456789Z"),
        "synthetic-a",
    );
    let parsed = Login::parse(&complete).unwrap();
    assert_eq!(parsed.account_id, "synthetic-account");
    assert_eq!(
        parsed.account,
        Account {
            id: "synthetic-account".into(),
            email: Some("person@example.test".into()),
            plan: Some("plus".into()),
        }
    );
    let mut value: serde_json::Value = serde_json::from_slice(&complete).unwrap();
    for broken in [
        serde_json::json!({"auth_mode": "apikey", "OPENAI_API_KEY": "synthetic"}),
        serde_json::json!({"auth_mode": "chatgpt"}),
        {
            value["tokens"]["refresh_token"] = "".into();
            value.clone()
        },
        {
            value["tokens"]["refresh_token"] = "synthetic".into();
            value["tokens"]["account_id"] = serde_json::Value::Null;
            value
        },
    ] {
        assert!(
            Login::parse(&serde_json::to_vec(&broken).unwrap()).is_none(),
            "{broken}"
        );
    }
    assert!(Login::parse(b"{half written").is_none());
}

#[test]
fn an_unreadable_id_token_still_counts_without_an_account_name() {
    let mut value: serde_json::Value = serde_json::from_slice(&login("synthetic-account", None, "a")).unwrap();
    value["tokens"]["id_token"] = "not-a-token".into();
    let parsed = Login::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        parsed.account,
        Account {
            id: "synthetic-account".into(),
            ..Account::default()
        }
    );
}

#[test]
fn only_a_later_renewal_of_the_same_account_renews() {
    let parse = |account, refreshed| Login::parse(&login(account, refreshed, "synthetic")).unwrap();
    let base = parse("a", Some("2026-10-01T08:00:00.123456789Z"));
    // Nanoseconds count, and offsets compare as instants.
    assert!(parse("a", Some("2026-10-01T08:00:00.12345679Z")).renews(&base));
    assert!(parse("a", Some("2026-10-01T10:00:01+02:00")).renews(&base));
    assert!(!parse("a", Some("2026-10-01T08:00:00.123456789Z")).renews(&base));
    assert!(!parse("a", Some("2026-09-30T08:00:00Z")).renews(&base));
    assert!(!parse("b", Some("2026-10-09T08:00:00Z")).renews(&base));
    assert!(!parse("a", None).renews(&base));
    assert!(parse("a", Some("2026-09-30T08:00:00Z")).renews(&parse("a", None)));
}

#[test]
fn a_worker_renewal_replaces_the_login_here_and_nothing_else_does() {
    let home = private_home();
    let local = login("a", Some("2026-10-01T08:00:00Z"), "synthetic-a");
    stored(home.path(), &local);
    for reply in [
        local.clone(),
        login("a", Some("2026-09-01T08:00:00Z"), "synthetic-old"),
        login("b", Some("2026-10-09T08:00:00Z"), "synthetic-other"),
    ] {
        adopt(home.path(), &reply).unwrap();
        assert_eq!(std::fs::read(home.path().join(FILE)).unwrap(), local);
    }
    assert!(adopt(home.path(), b"{\"auth_mode\":\"apikey\"}").is_err());
    assert_eq!(std::fs::read(home.path().join(FILE)).unwrap(), local);

    let renewed = login("a", Some("2026-10-05T08:00:00Z"), "synthetic-b");
    adopt(home.path(), &renewed).unwrap();
    assert_eq!(std::fs::read(home.path().join(FILE)).unwrap(), renewed);

    // A sign-in that finished during the exchange is newer than the worker's renewal.
    let signed_in = login("a", Some("2026-10-06T08:00:00Z"), "synthetic-c");
    stored(home.path(), &signed_in);
    adopt(home.path(), &login("a", Some("2026-10-05T12:00:00Z"), "synthetic-d")).unwrap();
    assert_eq!(std::fs::read(home.path().join(FILE)).unwrap(), signed_in);

    // After a sign-out, nothing comes back.
    std::fs::remove_file(home.path().join(FILE)).unwrap();
    adopt(home.path(), &renewed).unwrap();
    assert!(!home.path().join(FILE).exists());
}

#[cfg(unix)]
#[test]
fn a_login_file_others_can_read_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let home = private_home();
    stored(home.path(), &login("a", None, "synthetic"));
    assert!(account(home.path()).unwrap().is_some());
    std::fs::set_permissions(home.path().join(FILE), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(account(home.path()).is_err());
}

#[test]
fn no_login_file_is_no_account() {
    let home = private_home();
    assert_eq!(account(home.path()).unwrap(), None);
    assert_eq!(account(&home.path().join("missing")).unwrap(), None);
}

#[test]
fn the_worker_command_runs_the_install_script_as_the_agent_user() {
    let command = install_command();
    assert!(command.contains("horizon-worker-tailnet agent"));
    assert!(command.contains("python3 -c"));
    assert!(command.ends_with("'\"'\"'/workspace/home'\"'\"''; fi"), "{command}");
}

/// Runs the real install script against a local stand-in for the worker's home.
#[cfg(unix)]
#[test]
fn an_exchange_gives_the_worker_the_login_and_takes_back_its_renewal() {
    let home = private_home();
    let worker = tempfile::tempdir().unwrap();
    let installed = worker.path().join(".codex/auth.json");
    let cancel = crate::cloud_runtime::Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let install = || {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(install_script(worker.path().to_str().unwrap()));
        command
    };
    let local = login("a", Some("2026-10-01T08:00:00Z"), "synthetic-a");
    stored(home.path(), &local);

    // A new worker gets the login as it is.
    exchange(home.path(), &mut install(), &runner).unwrap();
    assert_eq!(std::fs::read(&installed).unwrap(), local);

    // The worker renewed it while this computer was away: the renewal stays there and
    // replaces the spent copy here.
    let renewed = login("a", Some("2026-10-06T08:00:00.5Z"), "synthetic-b");
    std::fs::write(&installed, &renewed).unwrap();
    exchange(home.path(), &mut install(), &runner).unwrap();
    assert_eq!(std::fs::read(&installed).unwrap(), renewed);
    assert_eq!(std::fs::read(home.path().join(FILE)).unwrap(), renewed);

    // A later sign-in here, also of another account, replaces the worker's login.
    let other = login("b", Some("2026-10-07T08:00:00Z"), "synthetic-c");
    stored(home.path(), &other);
    exchange(home.path(), &mut install(), &runner).unwrap();
    assert_eq!(std::fs::read(&installed).unwrap(), other);
    assert_eq!(std::fs::read(home.path().join(FILE)).unwrap(), other);

    // Without a login here nothing runs, and the worker keeps what it has.
    std::fs::remove_file(home.path().join(FILE)).unwrap();
    let mut failing = Command::new("/bin/false");
    exchange(home.path(), &mut failing, &runner).unwrap();
    assert_eq!(std::fs::read(&installed).unwrap(), other);
}

#[cfg(unix)]
mod codex_cli {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in `codex` that appends each call's arguments to `directory/calls` and keeps
    /// the `OPENAI_API_KEY` it saw in `directory/env`; `body` runs only for `login`.
    fn fake(directory: &Path, body: &str) -> String {
        let path = directory.join("bin/codex");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let calls = directory.join("calls");
        let env = directory.join("env");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\n[ \"$1\" = probe ] && exit 0\nprintf '%s\\n' \"$*\" >> '{}'\nprintf '%s' \"${{OPENAI_API_KEY-unset}}\" > '{}'\n[ \"$1\" = login ] || {{ {logout} }}\n{body}\n",
                calls.display(),
                env.display(),
                logout = if body.contains("exit 1") { "echo 'Error revoking: synthetic' >&2; exit 1;" } else { "exit 0;" },
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Another test thread that forks while this file is open for writing keeps it busy
        // until that child execs; wait for that before a test runs it.
        for _ in 0..200 {
            match std::process::Command::new(&path).arg("probe").status() {
                Err(error) if error.raw_os_error() == Some(26) => std::thread::sleep(Duration::from_millis(10)),
                _ => break,
            }
        }
        path.to_str().unwrap().to_owned()
    }

    /// The account the settings show for account ID `id`.
    fn shown(id: &str) -> Account {
        Account {
            id: id.into(),
            ..Account::default()
        }
    }

    #[test]
    fn sign_out_refuses_an_account_another_window_signed_in_to() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex");
        private_directory(&home).unwrap();
        let other = login("b", Some("2026-10-02T08:00:00Z"), "synthetic-other");
        stored(&home, &other);
        let program = fake(root.path(), "exit 0");
        assert!(sign_out_with(&program, &home, &shown("a")).is_err());
        assert_eq!(std::fs::read(home.join(FILE)).unwrap(), other);
        assert!(calls(root.path()).is_empty(), "nothing is revoked");
        // A later renewal of the account shown signs out.
        sign_out_with(&program, &home, &shown("b")).unwrap();
        assert_eq!(account(&home).unwrap(), None);
    }

    fn calls(directory: &Path) -> Vec<String> {
        std::fs::read_to_string(directory.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn sign_in_runs_codex_login_into_the_private_home_with_file_storage() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex");
        private_directory(&home).unwrap();
        // An earlier login, as another Horizon window may have left it.
        let earlier = login("a", Some("2026-09-01T08:00:00Z"), "synthetic-earlier");
        stored(&home, &earlier);
        let held = root.path().join("earlier.json");
        std::fs::write(&held, &earlier).unwrap();
        let source = root.path().join("login.json");
        let new = login("a", Some("2026-10-01T08:00:00Z"), "synthetic");
        std::fs::write(&source, &new).unwrap();
        // Codex signs in into a staging home: the login of `home` stays as it is meanwhile.
        let program = fake(
            root.path(),
            &format!(
                "echo 'Starting local login server' >&2\numask 077\ncmp -s \"$CODEX_HOME/../auth.json\" '{}' && touch \"$CODEX_HOME/../earlier-kept\"\ncp '{}' \"$CODEX_HOME/auth.json\"",
                held.display(),
                source.display()
            ),
        );
        let account = sign_in_with(&program, &home, &crate::cloud_runtime::Cancellation::default()).unwrap();
        assert_eq!(account.email.as_deref(), Some("person@example.test"));
        // The earlier login is revoked last, with `codex logout`.
        assert_eq!(
            calls(root.path()),
            ["login -c cli_auth_credentials_store=\"file\"", "logout"]
        );
        assert_eq!(std::fs::read_to_string(root.path().join("env")).unwrap(), "unset");
        assert!(home.join("earlier-kept").exists());
        assert_eq!(std::fs::read(home.join(FILE)).unwrap(), new);
        assert_eq!(
            std::fs::metadata(home.join(FILE)).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::metadata(&home).unwrap().permissions().mode() & 0o777, 0o700);
        // The staging home goes with the sign-in.
        let left: Vec<_> = std::fs::read_dir(&home)
            .unwrap()
            .filter_map(|entry| entry.unwrap().file_name().into_string().ok())
            .filter(|name| name.starts_with(".sign-in-"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn a_failed_sign_in_reports_the_error_codex_printed() {
        let root = tempfile::tempdir().unwrap();
        let program = fake(
            root.path(),
            "echo 'Starting local login server' >&2\necho 'Error logging in: Port 127.0.0.1:1457 is already in use' >&2\nexit 1",
        );
        let error = sign_in_with(
            &program,
            &root.path().join("codex"),
            &crate::cloud_runtime::Cancellation::default(),
        )
        .unwrap_err()
        .to_string();
        assert_eq!(error, "Codex: Error logging in: Port 127.0.0.1:1457 is already in use");
    }

    #[test]
    fn a_sign_in_without_a_login_file_or_without_codex_fails_plainly() {
        let root = tempfile::tempdir().unwrap();
        let program = fake(root.path(), "exit 0");
        let home = root.path().join("codex");
        let cancel = crate::cloud_runtime::Cancellation::default();
        assert_eq!(
            sign_in_with(&program, &home, &cancel).unwrap_err().to_string(),
            "Codex finished without a ChatGPT login"
        );
        let missing = root.path().join("no-codex");
        assert!(matches!(
            sign_in_with(missing.to_str().unwrap(), &home, &cancel),
            Err(SignInError::NoCodex)
        ));
    }

    #[test]
    fn a_cancelled_sign_in_stops_codex() {
        let root = tempfile::tempdir().unwrap();
        let program = fake(root.path(), "sleep 30");
        let cancel = crate::cloud_runtime::Cancellation::default();
        let stopper = cancel.clone();
        let started = std::time::Instant::now();
        let waiting = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            stopper.cancel();
        });
        assert!(sign_in_with(&program, &root.path().join("codex"), &cancel).is_err());
        waiting.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn sign_out_runs_codex_logout_and_removes_what_it_left() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex");
        private_directory(&home).unwrap();
        stored(&home, &login("a", None, "synthetic"));
        // A failed revocation leaves the file to Horizon; the sign-out still ends here.
        let program = fake(root.path(), "exit 1");
        sign_out_with(&program, &home, &shown("a")).unwrap();
        assert_eq!(calls(root.path()), ["logout"]);
        assert_eq!(account(&home).unwrap(), None);
        // Without Codex the login goes too.
        stored(&home, &login("a", None, "synthetic"));
        sign_out_with(root.path().join("no-codex").to_str().unwrap(), &home, &shown("a")).unwrap();
        assert_eq!(account(&home).unwrap(), None);
        // Without a login there is nothing to run.
        sign_out_with(&program, &home, &shown("a")).unwrap();
        assert_eq!(calls(root.path()), ["logout"]);
    }

    #[test]
    fn abandoned_sign_ins_go_and_one_under_way_stays() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex");
        private_directory(&home).unwrap();
        let abandoned = home.join(".sign-in-abandoned");
        let current = home.join(".sign-in-current");
        for staging in [&abandoned, &current] {
            std::fs::create_dir(staging).unwrap();
            std::fs::write(staging.join(FILE), b"synthetic").unwrap();
        }
        let unfinished = home.join(".auth-unfinished");
        std::fs::write(&unfinished, b"synthetic").unwrap();
        let old = std::time::SystemTime::now() - SIGN_IN - Duration::from_mins(6);
        std::fs::File::open(&abandoned).unwrap().set_modified(old).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&unfinished)
            .unwrap()
            .set_modified(old)
            .unwrap();
        sign_out_with(root.path().join("no-codex").to_str().unwrap(), &home, &shown("a")).unwrap();
        assert!(!abandoned.exists() && !unfinished.exists());
        assert!(current.join(FILE).exists());
    }

    #[test]
    fn a_login_file_others_can_read_still_signs_out() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex");
        private_directory(&home).unwrap();
        stored(&home, &login("a", None, "synthetic"));
        std::fs::set_permissions(home.join(FILE), std::fs::Permissions::from_mode(0o644)).unwrap();
        sign_out_with(root.path().join("no-codex").to_str().unwrap(), &home, &shown("a")).unwrap();
        assert!(!home.join(FILE).exists());
    }
}

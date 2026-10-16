use super::*;
use crate::cloud_runtime::Cancellation;
use base64::Engine as _;
use std::time::{Duration, SystemTime};

fn account(id: u64, login: &str) -> Account {
    Account {
        id,
        login: login.into(),
    }
}

#[test]
fn account_identity_includes_the_immutable_provider_id() {
    let original = account(10, "alice");
    assert!(original.check(&account(10, "Alice")).is_ok());
    assert!(
        original.check(&account(11, "alice")).is_err(),
        "a reused name is not the account"
    );
    assert!(
        original.check(&account(10, "bob")).is_err(),
        "a rename cannot redirect the namespace"
    );
    assert!(account(0, "alice").check(&account(0, "alice")).is_err());
}

#[test]
fn an_account_mismatch_is_refused_before_preparing_a_login() {
    let expected = account(10, "alice");
    let result = prepare_with(Some(&expected), || {
        Ok((account(11, "bob"), Secret::new("synthetic-other-token".into())))
    });
    assert!(result.is_err());
}

#[test]
fn simultaneous_operations_keep_their_own_logins_and_leave_the_host_config_intact() {
    let root = tempfile::tempdir().unwrap();
    let original = br#"{"auths":{"registry.example":{"auth":"c3ludGhldGlj"}}}"#;
    super::super::write_private(root.path(), "config.json", original).unwrap();
    let first = prepare_with(Some(&account(10, "alice")), || {
        Ok((account(10, "alice"), Secret::new("synthetic-first-token".into())))
    })
    .unwrap();
    let second = prepare_with(Some(&account(11, "bob")), || {
        Ok((account(11, "bob"), Secret::new("synthetic-second-token".into())))
    })
    .unwrap();
    let read = |prepared: &Prepared| {
        let bytes = std::fs::read(prepared.docker_config().join("config.json")).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        base64::engine::general_purpose::STANDARD
            .decode(json["auths"]["ghcr.io"]["auth"].as_str().unwrap())
            .unwrap()
    };
    assert_eq!(read(&first), b"alice:synthetic-first-token");
    assert_eq!(read(&second), b"bob:synthetic-second-token");
    assert_ne!(first.docker_config(), second.docker_config());
    assert_eq!(std::fs::read(root.path().join("config.json")).unwrap(), original);
    let first_path = first.docker_config().to_path_buf();
    drop(first);
    assert!(!first_path.exists());
    assert!(second.docker_config().is_dir());
}

#[test]
fn a_cancelled_preparation_creates_no_login_folder_or_prompt() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cancelled-login");
    let cancel = Cancellation::default();
    cancel.cancel();
    let events = std::cell::RefCell::new(Vec::new());
    let emit = |event| events.borrow_mut().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    assert!(prepare(&path, "cancelled-cloud", None, &runner).is_err());
    assert!(!path.exists());
    assert!(events.borrow().is_empty());
}

#[test]
#[cfg(unix)]
fn temporary_logins_are_private_and_account_metadata_contains_no_token() {
    use std::os::unix::fs::PermissionsExt;
    let prepared = prepare_with(None, || {
        Ok((account(7, "alice"), Secret::new("synthetic-private-token".into())))
    })
    .unwrap();
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(prepared.docker_config()), 0o700);
    assert_eq!(mode(&prepared.docker_config().join("config.json")), 0o600);
    let metadata = serde_json::to_string(prepared.account()).unwrap();
    assert!(!metadata.contains("synthetic-private-token"));
    assert!(!metadata.contains("auth"));
    assert!(!format!("{:?}", Secret::new("synthetic-private-token".into())).contains("synthetic-private-token"));
}

#[test]
fn a_renewed_chain_keeps_the_provider_identity_and_stores_rotation_before_use() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(STORE);
    stored::save(
        &path,
        "alice",
        &horizon_cloud::github::Chain {
            access_token: Secret::new("old-access".into()),
            access_expires_at: SystemTime::now() + Duration::from_secs(60),
            refresh_token: Secret::new("old-refresh".into()),
            refresh_expires_at: SystemTime::now() + Duration::from_hours(24),
        },
        false,
    )
    .unwrap();
    let (client, task) = crate::cloud_runtime::github::tests::github(vec![
        serde_json::json!({"access_token":"new-access","expires_in":28_800,"refresh_token":"new-refresh","refresh_token_expires_in":15_724_800,"token_type":"bearer","scope":"write:packages"}),
        serde_json::json!({"id":77,"login":"alice"}),
    ]);
    let (user, token) = stored::current_user(&path, &client, CLIENT_ID, None)
        .map_err(|_| "renewal failed")
        .unwrap()
        .unwrap();
    task.join().unwrap();
    assert_eq!(user.id, 77);
    assert_eq!(user.login, "alice");
    assert_eq!(token.expose(), "new-access");
    assert_eq!(stored::load(&path).unwrap().refresh_token.expose(), "new-refresh");
}

#[test]
fn renewal_keeps_the_same_account_and_another_account_changes_no_operation_login() {
    let prepared = prepare_with(None, || {
        Ok((account(7, "alice"), Secret::new("synthetic-first".into())))
    })
    .unwrap();
    let path = prepared.docker_config().join("config.json");
    let before = std::fs::read(&path).unwrap();
    assert!(
        prepared
            .renew_with(&account(8, "bob"), &Secret::new("synthetic-other".into()))
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    prepared
        .renew_with(&account(7, "alice"), &Secret::new("synthetic-renewed".into()))
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(json["auths"]["ghcr.io"]["auth"].as_str().unwrap())
            .unwrap(),
        b"alice:synthetic-renewed"
    );
}

#[test]
fn cancellation_after_authentication_returns_no_prepared_login() {
    let root = tempfile::tempdir().unwrap();
    stored::save(
        &root.path().join(STORE),
        "alice",
        &horizon_cloud::github::Chain {
            access_token: Secret::new("synthetic-access".into()),
            access_expires_at: SystemTime::now() + Duration::from_hours(4),
            refresh_token: Secret::new("synthetic-refresh".into()),
            refresh_expires_at: SystemTime::now() + Duration::from_hours(24),
        },
        false,
    )
    .unwrap();
    let (client, task) = crate::cloud_runtime::github::tests::github(vec![serde_json::json!({"id":7,"login":"alice"})]);
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let directory = std::cell::RefCell::new(None);
    let result = with_credentials_using(
        root.path(),
        "cancelled-cloud",
        None,
        &runner,
        &client,
        |account, token| {
            let prepared = prepare_with(None, || Ok((account, token)))?;
            directory.replace(Some(prepared.docker_config().to_path_buf()));
            cancel.cancel();
            Ok(prepared)
        },
    );
    task.join().unwrap();
    assert!(result.is_err());
    assert!(!directory.borrow().as_ref().unwrap().exists());
}

#[test]
fn cancellation_during_a_revoked_lookup_makes_no_sign_in_request_or_prompt() {
    use std::io::Write as _;
    let root = tempfile::tempdir().unwrap();
    stored::save(
        &root.path().join(STORE),
        "alice",
        &horizon_cloud::github::Chain {
            access_token: Secret::new("synthetic-access".into()),
            access_expires_at: SystemTime::now() + Duration::from_hours(4),
            refresh_token: Secret::new("synthetic-refresh".into()),
            refresh_expires_at: SystemTime::now() + Duration::from_hours(24),
        },
        false,
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client = Client::loopback(listener.local_addr().unwrap()).unwrap();
    let cancel = Cancellation::default();
    let stop = cancel.clone();
    let task = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        crate::cloud_runtime::github::tests::read_request(&mut stream);
        stop.cancel();
        write!(
            stream,
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        )
        .unwrap();
        drop(stream);
        listener.set_nonblocking(true).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < until {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    crate::cloud_runtime::github::tests::read_request(&mut stream);
                    let body = serde_json::json!({"device_code":"synthetic-device","user_code":"WDJB-MJHT","verification_uri":"https://github.com/login/device","expires_in":900,"interval":1}).to_string();
                    write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                    return 2;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{error}"),
            }
        }
        1
    });
    let events = std::cell::RefCell::new(Vec::new());
    let emit = |event| events.borrow_mut().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    let result: Result<()> =
        with_credentials_using(root.path(), "cancel-revoked-lookup", None, &runner, &client, |_, _| {
            panic!("a stopped operation must not use credentials")
        });
    assert!(matches!(
        result,
        Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
    ));
    assert_eq!(task.join().unwrap(), 1, "no device-code request follows cancellation");
    assert!(
        events.borrow().is_empty(),
        "no new authorization prompt or output follows cancellation"
    );
}

#[test]
fn cancellation_during_device_code_lookup_does_not_display_the_code() {
    use std::io::Write as _;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client = Client::loopback(listener.local_addr().unwrap()).unwrap();
    let cancel = Cancellation::default();
    let stop = cancel.clone();
    let task = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        crate::cloud_runtime::github::tests::read_request(&mut stream);
        stop.cancel();
        let body = serde_json::json!({"device_code":"synthetic-device","user_code":"WDJB-MJHT","verification_uri":"https://github.com/login/device","expires_in":900,"interval":1}).to_string();
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
    });
    let events = std::cell::RefCell::new(Vec::new());
    let emit = |event| events.borrow_mut().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    let result = super::super::signin::device_chain(
        &client,
        CLIENT_ID,
        Some("write:packages"),
        "cancel-device-code",
        &runner,
        |user_code, verification_uri, expires_at| super::super::Prompt::Publish {
            user_code,
            verification_uri,
            expires_at,
        },
    );
    task.join().unwrap();
    assert!(matches!(
        result,
        Err(super::super::signin::Ended::Error(Error::Provider(
            horizon_cloud::CloudError::Cancelled
        )))
    ));
    assert!(
        events.borrow().is_empty(),
        "a stopped lookup must not display a new code"
    );
}

#[test]
fn a_cancelled_token_or_account_answer_ends_the_prompt_without_saving_a_chain() {
    use std::io::Write as _;
    for account_lookup in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = Client::loopback(listener.local_addr().unwrap()).unwrap();
        let cancel = Cancellation::default();
        let stop = cancel.clone();
        let task = std::thread::spawn(move || {
            let mut replies = vec![
                serde_json::json!({"device_code":"synthetic-device","user_code":"WDJB-MJHT","verification_uri":"https://github.com/login/device","expires_in":900,"interval":1}),
                serde_json::json!({"access_token":"synthetic-access","expires_in":28_800,"refresh_token":"synthetic-refresh","refresh_token_expires_in":15_724_800,"token_type":"bearer","scope":"write:packages"}),
            ];
            if account_lookup {
                replies.push(serde_json::json!({"id":10,"login":"alice"}));
            }
            let expected = replies.len();
            for (index, body) in replies.into_iter().enumerate() {
                let (mut stream, _) = listener.accept().unwrap();
                crate::cloud_runtime::github::tests::read_request(&mut stream);
                if index + 1 == expected {
                    stop.cancel();
                }
                let body = body.to_string();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
            listener.set_nonblocking(true).unwrap();
            let until = std::time::Instant::now() + Duration::from_secs(2);
            while std::time::Instant::now() < until {
                match listener.accept() {
                    Ok(_) => return expected + 1,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            }
            expected
        });
        let events = std::cell::RefCell::new(Vec::new());
        let emit = |event| events.borrow_mut().push(event);
        let runner = Runner {
            cancel: &cancel,
            emit: &emit,
            secrets: Vec::new(),
        };
        let result = super::super::sign_in_as(
            root.path(),
            &client,
            "cancel-token-or-account",
            Some(&account(10, "alice")),
            &runner,
        );
        assert!(matches!(
            result,
            Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
        ));
        assert_eq!(task.join().unwrap(), if account_lookup { 3 } else { 2 });
        assert!(matches!(
            events.borrow().last(),
            Some(crate::cloud_runtime::Event::GitHub(super::super::Prompt::Published {
                allowed: false
            }))
        ));
        assert!(
            !root.path().join(STORE).exists(),
            "a cancelled answer must not save a credential chain"
        );
    }
}

fn retained_case(correct: bool, renew: bool) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(STORE);
    stored::save(
        &path,
        "bob",
        &horizon_cloud::github::Chain {
            access_token: Secret::new("bob-access".into()),
            access_expires_at: SystemTime::now() + Duration::from_hours(8),
            refresh_token: Secret::new("bob-refresh".into()),
            refresh_expires_at: SystemTime::now() + Duration::from_hours(24),
        },
        false,
    )
    .unwrap();
    let original = std::fs::read(&path).unwrap();
    let (client, task) = crate::cloud_runtime::github::tests::github(vec![
        serde_json::json!({"id":11,"login":"bob"}),
        serde_json::json!({"device_code":"synthetic-device","user_code":"WDJB-MJHT",
                "verification_uri":"https://github.com/login/device","expires_in":900,"interval":1}),
        serde_json::json!({"access_token":"new-access","expires_in":28_800,"refresh_token":"new-refresh",
                "refresh_token_expires_in":15_724_800,"token_type":"bearer","scope":"write:packages"}),
        serde_json::json!({"id":if correct {10} else {11},"login":"alice"}),
    ]);
    let cancel = Cancellation::default();
    let events = std::cell::RefCell::new(Vec::new());
    let emit = |event| events.borrow_mut().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    let expected = account(10, "alice");
    let prepared = prepare_with(Some(&expected), || {
        Ok((expected.clone(), Secret::new("original-operation-token".into())))
    })
    .unwrap();
    let private_before = std::fs::read(prepared.docker_config().join("config.json")).unwrap();
    let result = if renew {
        prepared
            .renew_using(root.path(), "recorded-account", &runner, &client)
            .map(|()| expected.clone())
    } else {
        prepare_using(root.path(), "recorded-account", Some(&expected), &runner, &client)
            .map(|prepared| prepared.account().clone())
    };
    task.join().unwrap();
    assert!(events.borrow().iter().any(|event| matches!(
        event,
        crate::cloud_runtime::Event::GitHub(super::super::Prompt::Publish { .. })
    )));
    if correct {
        assert_eq!(result.unwrap(), expected);
        if renew {
            assert_ne!(
                std::fs::read(prepared.docker_config().join("config.json")).unwrap(),
                private_before
            );
        }
        assert_eq!(stored::load(&path).unwrap().login, "alice");
    } else {
        assert!(result.is_err());
        assert!(matches!(
            events.borrow().last(),
            Some(crate::cloud_runtime::Event::GitHub(super::super::Prompt::Published {
                allowed: false
            }))
        ));
        assert_eq!(
            std::fs::read(prepared.docker_config().join("config.json")).unwrap(),
            private_before
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "a refused sign-in preserves the host's other account"
        );
    }
}

#[test]
fn a_recorded_account_can_sign_in_without_accepting_or_erasing_another_actor() {
    for correct in [false, true] {
        retained_case(correct, false);
    }
}

#[test]
fn renewal_recovers_only_the_recorded_account_without_replacing_another_login() {
    for correct in [false, true] {
        retained_case(correct, true);
    }
}

#[test]
fn explicit_account_selection_does_not_reuse_a_cached_publisher() {
    let root = tempfile::tempdir().unwrap();
    stored::save(
        &root.path().join(STORE),
        "bob",
        &horizon_cloud::github::Chain {
            access_token: Secret::new("bob-access".into()),
            access_expires_at: SystemTime::now() + Duration::from_hours(8),
            refresh_token: Secret::new("bob-refresh".into()),
            refresh_expires_at: SystemTime::now() + Duration::from_hours(24),
        },
        false,
    )
    .unwrap();
    let (client, task) = crate::cloud_runtime::github::tests::github(vec![
        serde_json::json!({"device_code":"synthetic-device","user_code":"WDJB-MJHT","verification_uri":"https://github.com/login/device","expires_in":900,"interval":1}),
        serde_json::json!({"access_token":"new-access","expires_in":28_800,"refresh_token":"new-refresh","refresh_token_expires_in":15_724_800,"token_type":"bearer","scope":"write:packages"}),
        serde_json::json!({"id":10,"login":"Alice"}),
    ]);
    let cancel = Cancellation::default();
    let events = std::cell::RefCell::new(Vec::new());
    let emit = |event| events.borrow_mut().push(event);
    let selected = select_account_using(
        root.path(),
        "explicit-account",
        &Runner {
            cancel: &cancel,
            emit: &emit,
            secrets: Vec::new(),
        },
        &client,
    )
    .unwrap();
    task.join().unwrap();
    assert_eq!(selected, account(10, "alice"));
    assert_eq!(stored::load(&root.path().join(STORE)).unwrap().login, "Alice");
    assert!(events.borrow().iter().any(|event| matches!(
        event,
        crate::cloud_runtime::Event::GitHub(super::super::Prompt::Publish { .. })
    )));
}

#[test]
fn a_cached_renamed_account_is_refused_without_an_impossible_device_prompt() {
    let root = tempfile::tempdir().unwrap();
    stored::save(
        &root.path().join(STORE),
        "alice",
        &horizon_cloud::github::Chain {
            access_token: Secret::new("alice-access".into()),
            access_expires_at: SystemTime::now() + Duration::from_hours(8),
            refresh_token: Secret::new("alice-refresh".into()),
            refresh_expires_at: SystemTime::now() + Duration::from_hours(24),
        },
        false,
    )
    .unwrap();
    let original = std::fs::read(root.path().join(STORE)).unwrap();
    let (client, task) =
        crate::cloud_runtime::github::tests::github(vec![serde_json::json!({"id":10,"login":"alice-renamed"})]);
    let cancel = Cancellation::default();
    let events = std::cell::RefCell::new(Vec::new());
    let emit = |event| events.borrow_mut().push(event);
    let result = prepare_using(
        root.path(),
        "renamed-account",
        Some(&account(10, "alice")),
        &Runner {
            cancel: &cancel,
            emit: &emit,
            secrets: Vec::new(),
        },
        &client,
    );
    task.join().unwrap();
    assert!(matches!(result,Err(Error::Invalid(message)) if message.contains("renamed")));
    assert!(!events.borrow().iter().any(|event| matches!(
        event,
        crate::cloud_runtime::Event::GitHub(super::super::Prompt::Publish { .. })
    )));
    assert_eq!(std::fs::read(root.path().join(STORE)).unwrap(), original);
}

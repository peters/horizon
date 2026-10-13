use super::*;
use crate::test_egui::DiscardTextures as _;

fn account() -> Account {
    Account {
        root: PathBuf::from("/synthetic"),
        settings: github::Settings {
            app_id: 42,
            slug: "horizon-example".into(),
            client_id: "Iv23synthetic".into(),
            client_secret_file: PathBuf::from("/synthetic/secret"),
            mode: github::Mode::Ask,
        },
        job: None,
        prompt: None,
        repositories: None,
        failed: None,
        stale: None,
        listed_for: None,
        signed_in_as: None,
        retry_at: None,
        check_at: None,
        refreshed: true,
    }
}

#[test]
fn typing_narrows_the_repositories_by_name() {
    let names: Vec<String> = ["acme/api", "acme/web", "acme/web-docs", "octo/web"]
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(matching(&names, "").len(), 4);
    assert_eq!(matching(&names, "WEB"), [&names[1], &names[2], &names[3]]);
    assert_eq!(matching(&names, "acme/web"), [&names[1], &names[2]]);
    let many: Vec<String> = (0..20).map(|n| format!("acme/repo-{n}")).collect();
    assert_eq!(matching(&many, "repo").len(), SHOWN);
}

#[test]
fn a_private_github_link_offers_the_connected_account_unless_a_token_was_chosen() {
    let github = horizon_core::cloud_runtime::repository::source::parse("github.com/acme/private").unwrap();
    let gitlab = horizon_core::cloud_runtime::repository::source::parse("gitlab.com/acme/private").unwrap();
    let mut state = super::super::State::default();
    assert!(!state.offers_account(&github), "without a connected app");
    state.account = Some(account());
    assert!(state.offers_account(&github));
    assert!(!state.offers_account(&gitlab));
    state.token_instead = true;
    assert!(!state.offers_account(&github));
}

#[test]
fn the_list_shows_the_connected_repositories() {
    let mut account = account();
    account.repositories = Some(vec!["acme/web".into(), "acme/api".into()]);
    let texts: Vec<String> = egui::Context::default()
        .run_ui(egui::RawInput::default(), |ui| {
            assert!(account.picker(ui, "web").is_none(), "nothing is chosen without a click");
        })
        .discard_textures()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|text| text == "acme/web"));
    assert!(!texts.iter().any(|text| text == "acme/api"));
}

#[test]
fn ending_a_request_ends_only_its_own_sign_in() {
    let (mine, other) = (Cancellation::default(), Cancellation::default());
    drop(Abort(mine.clone()));
    assert!(mine.check().is_err());
    assert!(other.check().is_ok(), "another request's sign-in goes on");
}

#[test]
fn an_automatic_sign_in_offers_the_page_again_and_a_way_out() {
    let web = Prompt::Web {
        url: "https://github.com/login/oauth/authorize?client_id=Iv23synthetic".into(),
    };
    let texts: Vec<String> = egui::Context::default()
        .run_ui(egui::RawInput::default(), |ui| {
            assert!(!prompt(ui, Some(&web), true), "nothing is cancelled without a click");
        })
        .discard_textures()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|text| text == "Open GitHub"));
    assert!(texts.iter().any(|text| text == "Cancel"));
}

#[test]
fn a_token_asked_for_another_repository_starts_nothing() {
    let mut state = super::super::State::default();
    state.input = "github.com/acme/other".into();
    state.account_for = horizon_core::cloud_runtime::repository::source::parse("github.com/acme/private");
    let (sender, receiver) = channel();
    sender
        .send(Answer::Done {
            token: Secret::new("ghu_synthetic".into()),
            repositories: None,
        })
        .unwrap();
    let mut account = account();
    account.job = Some(Job {
        receiver,
        purpose: Purpose::Clone,
        _abort: Abort(Cancellation::default()),
    });
    state.account = Some(account);
    state.take_account_token(&egui::Context::default());
    assert!(state.token.is_empty(), "the token is not used");
    assert!(state.job.is_none() && !state.connected, "no clone started");
    assert!(state.account_for.is_none());
}

#[test]
fn a_typed_name_narrows_the_list_and_a_link_sets_it_aside() {
    assert!(!names_a_link("acme/web"));
    assert!(!names_a_link(" web "));
    for link in [
        "https://github.com/acme/web",
        "github.com/acme/web",
        "git@github.com:acme/web.git",
    ] {
        assert!(names_a_link(link), "{link}");
    }
    let mut state = super::super::State::default();
    let mut account = account();
    account.repositories = Some(vec!["acme/api".into(), "acme/web".into()]);
    state.account = Some(account);
    let texts = |state: &mut super::super::State| -> Vec<String> {
        egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| state.github_section(ui))
            .discard_textures()
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    };
    state.input = "acme/web".into();
    let _ = state.remote();
    let shown = texts(&mut state);
    assert!(shown.iter().any(|text| text == "acme/web"), "{shown:?}");
    assert!(!shown.iter().any(|text| text == "acme/api"));
    state.input = "https://github.com/acme/web".into();
    let _ = state.remote();
    assert!(!texts(&mut state).iter().any(|text| text == "acme/web"));
}

#[test]
fn a_name_that_narrows_the_list_is_no_unknown_link() {
    let mut state = super::super::State::default();
    state.input = "web".into();
    assert!(!state.filters_the_list(), "without a list");
    let mut account = account();
    account.repositories = Some(vec!["acme/web".into()]);
    state.account = Some(account);
    assert!(state.filters_the_list());
    state.input = "nothing-like-it".into();
    assert!(
        !state.filters_the_list(),
        "a name that matches nothing is still unknown"
    );
    state.input = "acme/we".into();
    assert!(state.filters_the_list());
    assert!(state.remote().is_none(), "a partial name is not looked up");
    state.input = "ACME/web".into();
    assert!(!state.filters_the_list(), "a listed repository in full is the choice");
    assert!(state.remote().is_some());
    state.input = "https://example.org/web".into();
    assert!(!state.filters_the_list(), "a link is never a filter");
}

#[test]
fn a_token_for_the_repository_in_the_field_is_used_once_and_never_kept() {
    let temp = tempfile::tempdir().unwrap();
    let remote = horizon_core::cloud_runtime::repository::source::parse("github.com/acme/private").unwrap();
    // A checkout of the link is already there, so the start takes it without the network.
    let checkout = temp.path().join("private");
    std::fs::create_dir(&checkout).unwrap();
    for args in [
        &["init", "-q"][..],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "first",
        ],
        &["remote", "add", "origin", &remote.url],
    ] {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&checkout)
            .args(args)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "{args:?}");
    }
    let mut state = super::super::State::default();
    state.set_parent(temp.path());
    state.input = "github.com/acme/private".into();
    state.remember = true;
    state.account_for = Some(remote.clone());
    let (sender, receiver) = channel();
    sender
        .send(Answer::Done {
            token: Secret::new("ghu_synthetic".into()),
            repositories: None,
        })
        .unwrap();
    let mut account = account();
    account.job = Some(Job {
        receiver,
        purpose: Purpose::Clone,
        _abort: Abort(Cancellation::default()),
    });
    state.account = Some(account);
    state.take_account_token(&egui::Context::default());
    assert!(state.connected, "the clone uses the connected account");
    assert!(!state.remember, "its token is never kept in Git's helper");
    assert!(
        horizon_core::cloud_runtime::repository::source::Token::new(&remote, &state.token).is_some(),
        "the token serves this repository's origin"
    );
    assert_eq!(state.ready.as_deref(), Some(checkout.as_path()), "the start ran");
    assert!(state.account_for.is_none());
}

/// An account waiting on a request of `purpose`, and the sender its thread would use.
fn waiting(purpose: Purpose) -> (Account, std::sync::mpsc::Sender<Answer>) {
    let (sender, receiver) = channel();
    let mut account = account();
    account.job = Some(Job {
        receiver,
        purpose,
        _abort: Abort(Cancellation::default()),
    });
    (account, sender)
}

#[test]
fn the_kept_list_shows_until_a_refresh_replaces_it() {
    let (mut account, sender) = waiting(Purpose::Refresh);
    assert!(!account.busy(), "a quiet refresh is no request of the person's");
    assert!(account.job.as_ref().is_some_and(|job| job.purpose == Purpose::Refresh));
    sender
        .send(Answer::Cached {
            login: "octo-cat".into(),
            repositories: vec!["acme/old".into()],
        })
        .unwrap();
    assert!(account.poll().is_none());
    assert_eq!(account.repositories.as_deref(), Some(&["acme/old".to_owned()][..]));
    assert!(
        account.job.as_ref().is_some_and(|job| job.purpose == Purpose::Refresh),
        "the refresh goes on behind the kept list"
    );
    sender
        .send(Answer::Done {
            token: Secret::new("ghu_synthetic".into()),
            repositories: Some(vec!["acme/new".into()]),
        })
        .unwrap();
    assert!(account.poll().is_none(), "a refresh hands no token over");
    assert_eq!(account.repositories.as_deref(), Some(&["acme/new".to_owned()][..]));
    assert!(account.job.as_ref().is_none_or(|job| job.purpose != Purpose::Refresh));
}

#[test]
fn a_failed_refresh_keeps_the_list_and_marks_it_out_of_date() {
    let (mut account, sender) = waiting(Purpose::Refresh);
    account.repositories = Some(vec!["acme/kept".into()]);
    sender
        .send(Answer::Failed("GitHub could not be reached.".into()))
        .unwrap();
    assert!(account.poll().is_none());
    assert_eq!(account.repositories.as_deref(), Some(&["acme/kept".to_owned()][..]));
    assert!(account.failed.is_none(), "no red error for a background refresh");
    assert_eq!(account.stale.as_deref(), Some("GitHub could not be reached."));
    assert!(account.job.is_none());
    let (mut account, sender) = waiting(Purpose::List);
    account.stale = Some("old".into());
    sender
        .send(Answer::Done {
            token: Secret::new("ghu_synthetic".into()),
            repositories: Some(vec!["acme/new".into()]),
        })
        .unwrap();
    assert!(account.poll().is_none());
    assert!(account.stale.is_none(), "a fresh list is current again");
}

#[test]
fn a_refresh_without_a_sign_in_shows_no_kept_list() {
    let (mut account, sender) = waiting(Purpose::Refresh);
    // Read just before the sign-in turned out to be gone.
    sender
        .send(Answer::Cached {
            login: "octo-cat".into(),
            repositories: vec!["acme/kept".into()],
        })
        .unwrap();
    sender.send(Answer::Quiet).unwrap();
    assert!(account.poll().is_none());
    assert!(account.repositories.is_none(), "the sign-in button stays in view");
    assert!(account.failed.is_none() && account.stale.is_none());
    assert!(account.job.is_none());
}

#[test]
fn a_failed_refresh_the_person_asked_for_keeps_the_way_to_try_again() {
    let (mut account, sender) = waiting(Purpose::List);
    account.repositories = Some(vec!["acme/kept".into()]);
    account.stale = Some("GitHub could not be reached.".into());
    sender.send(Answer::Failed("Still offline.".into())).unwrap();
    assert!(account.poll().is_none());
    assert!(account.stale.is_some(), "Refresh from GitHub stays in view");
    assert_eq!(account.failed.as_deref(), Some("Still offline."));
    assert!(account.retry_at.is_some(), "the quiet retries go on");
    let (sender, receiver) = channel();
    account.job = Some(Job {
        receiver,
        purpose: Purpose::Refresh,
        _abort: Abort(Cancellation::default()),
    });
    sender
        .send(Answer::Done {
            token: Secret::new("ghu_synthetic".into()),
            repositories: Some(vec!["acme/fresh".into()]),
        })
        .unwrap();
    assert!(account.poll().is_none());
    assert!(account.failed.is_none(), "a fresh list ends the old error");
}

#[test]
fn a_sign_in_as_another_account_never_shows_the_first_ones_list() {
    let (mut account, sender) = waiting(Purpose::List);
    account.repositories = Some(vec!["acme/private".into()]);
    account.listed_for = Some("octo-cat".into());
    account.stale = Some("old".into());
    sender.send(Answer::SignedIn("other-account".into())).unwrap();
    sender
        .send(Answer::Failed("GitHub could not be reached.".into()))
        .unwrap();
    assert!(account.poll().is_none());
    assert!(account.repositories.is_none(), "octo-cat's names are gone");
    assert!(account.stale.is_none());
    assert_eq!(account.failed.as_deref(), Some("GitHub could not be reached."));
    let (mut account, sender) = waiting(Purpose::List);
    account.repositories = Some(vec!["acme/web".into()]);
    account.listed_for = Some("octo-cat".into());
    sender.send(Answer::SignedIn("octo-cat".into())).unwrap();
    sender
        .send(Answer::Done {
            token: Secret::new("ghu_synthetic".into()),
            repositories: Some(vec!["acme/new".into()]),
        })
        .unwrap();
    assert!(account.poll().is_none());
    assert_eq!(
        account.listed_for.as_deref(),
        Some("octo-cat"),
        "the fresh list is the account's"
    );
}

#[test]
fn a_list_of_an_account_that_another_window_replaced_is_never_shown() {
    for purpose in [Purpose::Refresh, Purpose::List] {
        let (mut account, sender) = waiting(purpose);
        account.repositories = Some(vec!["acme/kept".into()]);
        account.listed_for = Some("octo-cat".into());
        sender.send(Answer::SignedIn("octo-cat".into())).unwrap();
        sender.send(Answer::Replaced).unwrap();
        assert!(account.poll().is_none());
        assert!(account.repositories.is_none(), "{:?}", account.repositories);
        assert!(account.stale.is_none());
        let expected = (purpose == Purpose::List).then_some(ANOTHER_ACCOUNT);
        assert_eq!(
            account.failed.as_deref(),
            expected,
            "only a request of the person's says so"
        );
    }
}

#[test]
fn an_out_of_date_list_tries_again_and_goes_when_another_account_is_signed_in() {
    let (mut account, sender) = waiting(Purpose::Refresh);
    sender
        .send(Answer::Cached {
            login: "octo-cat".into(),
            repositories: vec!["acme/kept".into()],
        })
        .unwrap();
    sender.send(Answer::SignedIn("octo-cat".into())).unwrap();
    sender
        .send(Answer::Failed("GitHub could not be reached.".into()))
        .unwrap();
    assert!(account.poll().is_none());
    assert!(account.stale.is_some());
    assert!(account.retry_at.is_some(), "a later try is planned");
    // The later try finds another account signed in, without the network.
    let (sender, receiver) = channel();
    account.job = Some(Job {
        receiver,
        purpose: Purpose::Refresh,
        _abort: Abort(Cancellation::default()),
    });
    sender.send(Answer::SignedIn("other-account".into())).unwrap();
    sender
        .send(Answer::Failed("GitHub could not be reached.".into()))
        .unwrap();
    assert!(account.poll().is_none());
    assert!(account.repositories.is_none(), "octo-cat's names are gone");
}

#[test]
fn a_refresh_that_finds_no_sign_in_drops_also_a_confirmed_list() {
    let (mut account, sender) = waiting(Purpose::Refresh);
    account.repositories = Some(vec!["acme/web".into()]);
    account.listed_for = Some("octo-cat".into());
    account.retry_at = Some(std::time::Instant::now());
    sender.send(Answer::Quiet).unwrap();
    assert!(account.poll().is_none());
    assert!(account.repositories.is_none());
    assert!(account.retry_at.is_none());
}

#[test]
fn a_shown_list_is_checked_against_the_signed_in_account_again() {
    let (mut account, sender) = waiting(Purpose::Check);
    assert!(!account.busy(), "a check is no request of the person's");
    account.repositories = Some(vec!["acme/web".into()]);
    account.listed_for = Some("octo-cat".into());
    sender.send(Answer::SignedIn("octo-cat".into())).unwrap();
    sender.send(Answer::Checked).unwrap();
    assert!(account.poll().is_none());
    assert!(account.repositories.is_some(), "the same account keeps its list");
    assert!(account.check_at.is_some(), "and is checked again later");
    let (mut account, sender) = waiting(Purpose::Check);
    account.repositories = Some(vec!["acme/web".into()]);
    account.listed_for = Some("octo-cat".into());
    sender.send(Answer::SignedIn("other-account".into())).unwrap();
    sender.send(Answer::Checked).unwrap();
    assert!(account.poll().is_none());
    assert!(account.repositories.is_none(), "another window's sign-in clears it");
}

#[test]
fn a_sign_in_that_turned_out_gone_drops_the_list_while_a_new_one_runs() {
    let (mut account, sender) = waiting(Purpose::List);
    account.repositories = Some(vec!["acme/private".into()]);
    account.listed_for = Some("octo-cat".into());
    account.stale = Some("old".into());
    sender.send(Answer::SignedOut).unwrap();
    assert!(account.poll().is_none());
    assert!(account.repositories.is_none(), "no names while the new sign-in runs");
    assert!(account.busy(), "the request goes on to sign in");
}

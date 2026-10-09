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
    state.input = "https://example.org/web".into();
    assert!(!state.filters_the_list(), "a link is never a filter");
}

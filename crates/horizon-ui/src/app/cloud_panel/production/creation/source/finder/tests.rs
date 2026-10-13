use super::{Kind, Pick, folder_rows, is_path, names_a_host, pick, search_rows};
use horizon_core::dir_search::Checkout;
use std::path::PathBuf;

fn checkout(path: &str, origin: Option<&str>) -> Checkout {
    Checkout {
        path: PathBuf::from(path),
        origin: origin.map(str::to_owned),
        branch: Some("main".into()),
        touched: None,
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn paths_links_and_words_are_told_apart() {
    for path in ["~", "~/code", "/srv/repo", "./here", "../there"] {
        assert!(is_path(path), "{path}");
    }
    for words in [
        "acme/web",
        "web",
        "https://github.com/acme/web",
        "git@github.com:acme/web.git",
    ] {
        assert!(!is_path(words), "{words}");
    }
}

#[test]
fn a_cloned_repository_shows_once_as_the_checkout_with_both_badges() {
    let checkouts = [checkout("/home/x/github/acme/web", Some("git@github.com:Acme/web.git"))];
    let rows = search_rows("web", &checkouts, &names(&["acme/web", "acme/website"]));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].title, "Acme/web", "as the checkout's own remote spells it");
    assert!(
        matches!(rows[0].kind, Kind::Local { github: true, .. }),
        "{:?}",
        rows[0].kind
    );
    assert!(matches!(&rows[1].kind, Kind::GitHub { name } if name == "acme/website"));
}

#[test]
fn before_typing_recent_checkouts_lead_then_the_account_list() {
    let checkouts: Vec<Checkout> = (0..10)
        .map(|n| checkout(&format!("/home/x/code/tool-{n}"), None))
        .collect();
    let rows = search_rows("", &checkouts, &names(&["acme/api"]));
    assert_eq!(rows.len(), 7, "six recent checkouts and the repository");
    assert_eq!(rows[0].title, "tool-0");
    assert!(matches!(rows[6].kind, Kind::GitHub { .. }));
}

#[test]
fn choosing_a_result_uses_a_checkout_clones_a_repository_or_opens_a_folder() {
    let rows = search_rows("api", &[], &names(&["acme/api"]));
    assert_eq!(pick(&rows[0], false), Pick::Text("https://github.com/acme/api".into()));
    let repo = PathBuf::from("/x/repo");
    let plain = PathBuf::from("/x/plain");
    let hidden = PathBuf::from("/x/.hidden");
    let rows = folder_rows("/x/", &[(plain, false), (hidden.clone(), false), (repo.clone(), true)]);
    assert_eq!(rows.len(), 2, "a hidden folder waits for a typed dot");
    assert_eq!(rows[0].title, "repo", "repositories come first");
    assert_eq!(pick(&rows[0], false), Pick::Checkout(repo.clone()));
    assert!(matches!(pick(&rows[0], true), Pick::Text(text) if text.ends_with(std::path::MAIN_SEPARATOR)));
    assert!(matches!(pick(&rows[1], false), Pick::Text(text) if text.ends_with(std::path::MAIN_SEPARATOR)));
    assert_eq!(folder_rows("/x/.h", &[(hidden, false)]).len(), 1);
}

#[test]
fn a_link_with_a_host_is_no_search() {
    assert!(names_a_host("gitlab.com/acme/api"));
    assert!(names_a_host("git.example.org/team/tool"));
    assert!(!names_a_host("acme/api"), "an owner/name is still a search");
    assert!(!names_a_host("web"));
    assert!(!names_a_host("./here"));
}

#[test]
fn letters_scattered_over_a_path_find_nothing() {
    let checkouts = [checkout("/home/x/projects/tool", None)];
    assert!(search_rows("wxyz", &checkouts, &[]).is_empty());
    assert_eq!(
        search_rows("projects/to", &checkouts, &[]).len(),
        1,
        "a run in the path still finds it"
    );
}

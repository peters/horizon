use super::{Kind, Pick, folder_rows, is_path, names_a_host, pick, search_rows};
use crate::test_egui::DiscardTextures as _;
use horizon_core::dir_search::Checkout;
use std::path::PathBuf;

/// The search rows for `query`, with the labels the finder makes when checkouts arrive.
fn rows_for(query: &str, checkouts: &[Checkout], repositories: &[String]) -> Vec<super::Row> {
    let labels: Vec<(String, String)> = checkouts
        .iter()
        .map(|checkout| (super::checkout_title(checkout), super::checkout_detail(checkout)))
        .collect();
    search_rows(query, checkouts, &labels, repositories)
}

fn checkout(path: &str, origin: Option<&str>) -> Checkout {
    Checkout {
        path: PathBuf::from(path),
        origin: origin.map(str::to_owned),
        branch: Some("main".into()),
        touched: None,
        complete: true,
        stopped: false,
        worktree: false,
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn paths_links_and_words_are_told_apart() {
    for path in ["~", "~/code", "/srv/repo", "./here", "../there", ".", ".."] {
        assert!(is_path(path), "{path}");
    }
    for words in [
        "acme/web",
        "web",
        ".github",
        "https://github.com/acme/web",
        "git@github.com:acme/web.git",
        "~alice",
    ] {
        assert!(!is_path(words), "{words}");
    }
    assert_eq!(is_path("\\\\server\\share"), cfg!(windows));
}

#[test]
fn a_cloned_repository_shows_once_as_the_checkout_with_both_badges() {
    let checkouts = [checkout("/home/x/github/acme/web", Some("git@github.com:Acme/web.git"))];
    let rows = rows_for("web", &checkouts, &names(&["acme/web", "acme/website"]));
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
    let rows = rows_for("", &checkouts, &names(&["acme/api"]));
    assert_eq!(rows.len(), 7, "six recent checkouts and the repository");
    assert_eq!(rows[0].title, "tool-0");
    assert!(matches!(rows[6].kind, Kind::GitHub { .. }));
}

#[test]
fn an_older_checkout_of_the_account_s_repository_keeps_its_place_in_the_account_list() {
    let mut checkouts: Vec<Checkout> = (0..6)
        .map(|n| checkout(&format!("/home/x/code/tool-{n}"), None))
        .collect();
    checkouts.push(checkout("/home/x/github/acme/api", Some("https://github.com/acme/api")));
    let mut empty = checkout("/home/x/trees/web-fix", Some("https://github.com/acme/web"));
    empty.complete = false;
    checkouts.push(empty);
    let rows = rows_for("", &checkouts, &names(&["acme/web", "acme/api", "acme/cli"]));
    let found: Vec<String> = rows[6..]
        .iter()
        .map(|row| match &row.kind {
            Kind::Local { path, github } => format!("{} {github}", path.display()),
            Kind::GitHub { name } => name.clone(),
            Kind::Folder { path, .. } => format!("folder {}", path.display()),
        })
        .collect();
    assert_eq!(
        found,
        [
            "acme/web",
            "/home/x/trees/web-fix true",
            "/home/x/github/acme/api true",
            "acme/cli"
        ],
        "after the six recent checkouts, in the account's order"
    );
    // A repository a newer checkout already stands in for is not listed again.
    checkouts.push(checkout("/home/x/trees/api-fix", Some("https://github.com/acme/api")));
    let rows = rows_for("", &checkouts, &names(&["acme/web", "acme/api", "acme/cli"]));
    assert_eq!(
        rows.len(),
        10,
        "{:?}",
        rows.iter().map(|row| &row.title).collect::<Vec<_>>()
    );
    checkouts[0] = checkout("/home/x/code/api", Some("https://github.com/acme/api"));
    let rows = rows_for("", &checkouts, &names(&["acme/web", "acme/api", "acme/cli"]));
    let api: Vec<&super::Row> = rows
        .iter()
        .filter(|row| row.repo.as_deref() == Some("acme/api"))
        .collect();
    assert_eq!(
        api.len(),
        1,
        "{:?}",
        rows.iter().map(|row| &row.title).collect::<Vec<_>>()
    );
    assert!(matches!(&api[0].kind, Kind::Local { path, .. } if path.ends_with("code/api")));
}

#[test]
fn choosing_a_result_uses_a_checkout_clones_a_repository_or_opens_a_folder() {
    let rows = rows_for("api", &[], &names(&["acme/api"]));
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
    assert!(rows_for("wxyz", &checkouts, &[]).is_empty());
    assert_eq!(
        rows_for("projects/to", &checkouts, &[]).len(),
        1,
        "a run in the path still finds it"
    );
}

#[test]
fn the_folder_named_as_typed_comes_before_a_repository_beside_it() {
    let rows = folder_rows(
        "~/code/tools",
        &[
            (PathBuf::from("/x/code/my-tools"), true),
            (PathBuf::from("/x/code/tools"), false),
        ],
    );
    assert_eq!(rows[0].title, "tools");
}

/// A state whose field holds `typed`, with one checkout to find, and the rows made.
fn searching(typed: &str) -> super::super::State {
    let mut state = super::super::State::default();
    state.input = typed.into();
    state.finder.checkouts = Some(vec![checkout(
        "/home/x/code/rust-analyzer",
        Some("https://github.com/rust-lang/rust-analyzer"),
    )]);
    let _ = state.finder.rows(typed, &[]);
    state
}

/// Presses `key` with `modifiers` in a frame where the field has the focus; returns what
/// the finder chose and whether the key was left for the field.
fn press(state: &mut super::super::State, key: egui::Key, modifiers: egui::Modifiers) -> (Option<PathBuf>, bool) {
    let ctx = egui::Context::default();
    let input = egui::RawInput {
        events: vec![egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }],
        ..Default::default()
    };
    let mut result = (None, false);
    let _ = ctx
        .run_ui(input, |ui| {
            let _ = state.remote();
            let chosen = state.finder_keys(ui, true);
            result = (chosen, ui.input(|input| input.key_pressed(key)));
        })
        .discard_textures();
    result
}

#[test]
fn enter_uses_the_highlighted_checkout() {
    let mut state = searching("analyzer");
    let (chosen, left) = press(&mut state, egui::Key::Enter, egui::Modifiers::NONE);
    assert_eq!(chosen, Some(PathBuf::from("/home/x/code/rust-analyzer")));
    assert!(!left, "the field never sees that Enter");
}

#[test]
fn a_typed_owner_and_name_leaves_enter_to_continue() {
    let mut state = searching("rust-lang/rust");
    let (chosen, left) = press(&mut state, egui::Key::Enter, egui::Modifiers::NONE);
    assert_eq!(chosen, None, "the look-alike checkout is not taken");
    assert!(left, "Enter goes on to clone rust-lang/rust");
}

#[test]
fn shift_tab_keeps_its_way_back() {
    let mut state = searching("~/");
    state.finder.rows = folder_rows("~/", &[(PathBuf::from("/x/code"), false)]);
    state.finder.folders_for = "~/".into();
    let (chosen, left) = press(&mut state, egui::Key::Tab, egui::Modifiers::SHIFT);
    assert_eq!(chosen, None);
    assert!(left, "Shift+Tab is not taken by the results");
}

#[test]
fn a_branch_or_an_age_finds_no_checkout() {
    let mut checkout = checkout("/home/x/code/tool", None);
    checkout.touched = Some(std::time::SystemTime::now() - std::time::Duration::from_secs(600));
    assert!(rows_for("main", std::slice::from_ref(&checkout), &[]).is_empty());
    assert!(rows_for("ago", std::slice::from_ref(&checkout), &[]).is_empty());
}

#[test]
fn new_rows_for_the_same_query_keep_the_highlighted_row() {
    let mut finder = super::Finder {
        checkouts: Some(Vec::new()),
        ..super::Finder::default()
    };
    let names = vec!["acme/api".to_owned(), "acme/web".to_owned()];
    let _ = finder.rows("", &names);
    finder.move_highlight(true);
    assert_eq!(finder.current_rows()[finder.highlighted].title, "acme/web");
    // The scan finishes and a recent checkout now leads the list.
    finder.checkouts = Some(vec![checkout("/home/x/code/tool", None)]);
    let _ = finder.rows("", &names);
    assert_eq!(
        finder.current_rows()[finder.highlighted].title,
        "acme/web",
        "still the row chosen"
    );
}

#[test]
fn arrows_choose_a_row_for_a_typed_owner_and_name() {
    let mut state = searching("rust-lang/rust");
    let _ = press(&mut state, egui::Key::ArrowDown, egui::Modifiers::NONE);
    let (chosen, left) = press(&mut state, egui::Key::Enter, egui::Modifiers::NONE);
    assert_eq!(
        chosen,
        Some(PathBuf::from("/home/x/code/rust-analyzer")),
        "the row picked with the arrows"
    );
    assert!(!left);
}

#[test]
fn letters_that_lower_to_more_than_one_still_light_their_own_place() {
    let found = super::fuzzy::score("w", "İacme/web").unwrap();
    assert_eq!(found.positions, [6]);
}

#[test]
fn a_typed_folder_that_is_a_repository_offers_itself_first() {
    let mut finder = super::Finder {
        checkouts: Some(Vec::new()),
        folders_for: "~/code/web/".into(),
        folders: (
            Some(PathBuf::from("/x/code/web")),
            vec![(PathBuf::from("/x/code/web/src"), false)],
        ),
        ..super::Finder::default()
    };
    let rows = finder.rows("~/code/web/", &[]).to_vec();
    assert_eq!(rows[0].title, "Use this repository");
    assert_eq!(pick(&rows[0], false), Pick::Checkout(PathBuf::from("/x/code/web")));
    assert_eq!(rows[1].title, "src");
}

#[test]
fn home_and_a_final_dot_list_the_right_folders() {
    assert_eq!(super::folders::folder_query("~"), "~/");
    assert_eq!(super::folders::folder_query(".."), "../");
    assert_eq!(
        super::folders::folder_query("~/."),
        "~/",
        "the home folder, its hidden folders shown"
    );
    assert_eq!(super::folders::folder_query("~/code/."), "~/code/");
    assert_eq!(super::folders::folder_query("~/code"), "~/code");
    assert_eq!(folder_rows("~/.", &[(PathBuf::from("/x/.config"), false)]).len(), 1);
}

#[test]
fn a_chosen_repository_stays_chosen_when_its_checkout_is_found() {
    let mut finder = super::Finder {
        checkouts: Some(vec![checkout("/home/x/code/tool", None)]),
        ..super::Finder::default()
    };
    let names = vec!["acme/api".to_owned(), "acme/web".to_owned()];
    let _ = finder.rows("acme", &names);
    while finder.current_rows()[finder.highlighted].title != "acme/web" {
        finder.move_highlight(true);
    }
    // The scan finds acme/web's checkout, which ranks ahead now.
    finder.checkouts = Some(vec![
        checkout("/home/x/code/tool", None),
        checkout("/home/x/github/acme/web", Some("https://github.com/acme/web")),
    ]);
    let _ = finder.rows("acme", &names);
    let row = &finder.current_rows()[finder.highlighted];
    assert!(matches!(row.kind, Kind::Local { .. }), "{row:?}");
    assert_eq!(row.repo.as_deref(), Some("acme/web"));
}

#[test]
fn a_repository_already_on_this_computer_is_found_by_its_origin() {
    let mut stopped = checkout("/home/x/stopped/web", Some("https://github.com/acme/web"));
    stopped.complete = false;
    let finder = super::Finder {
        checkouts: Some(vec![
            stopped,
            checkout("/home/x/elsewhere/web", Some("git@github.com:Acme/web.git")),
        ]),
        ..super::Finder::default()
    };
    let remote = horizon_core::cloud_runtime::repository::source::parse("https://github.com/acme/web").unwrap();
    assert_eq!(
        finder.checkout_of(&remote),
        Some(PathBuf::from("/home/x/elsewhere/web")),
        "a finished checkout, its origin written another way; not the stopped one"
    );
    let other = horizon_core::cloud_runtime::repository::source::parse("https://github.com/acme/api").unwrap();
    assert_eq!(finder.checkout_of(&other), None);
}

#[test]
fn a_stopped_clone_is_no_result_and_an_empty_checkout_hides_no_repository() {
    let mut stopped = checkout("/home/x/github/acme/web", Some("https://github.com/acme/web"));
    stopped.complete = false;
    stopped.stopped = true;
    // A checkout with no commit yet, such as a fresh `git init` given an origin.
    let mut empty = checkout("/home/x/trees/api-fix", Some("https://github.com/acme/api"));
    empty.complete = false;
    let rows = rows_for("", &[stopped, empty], &names(&["acme/web", "acme/api"]));
    let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
    assert!(
        !rows
            .iter()
            .any(|row| matches!(&row.kind, Kind::Local { path, .. } if path.ends_with("web"))),
        "the stopped clone is not offered: {titles:?}"
    );
    assert!(
        rows.iter()
            .any(|row| matches!(&row.kind, Kind::GitHub { name } if name == "acme/web")),
        "its repository's row clones it again: {titles:?}"
    );
    assert!(
        rows.iter()
            .any(|row| matches!(&row.kind, Kind::Local { path, .. } if path.ends_with("api-fix"))),
        "{titles:?}"
    );
    assert!(
        rows.iter()
            .any(|row| matches!(&row.kind, Kind::GitHub { name } if name == "acme/api")),
        "an empty checkout stands beside its repository's row: {titles:?}"
    );
}

#[test]
fn a_path_answered_again_makes_its_rows_again() {
    let mut finder = super::Finder {
        checkouts: Some(Vec::new()),
        folders_for: "~/code/web/".into(),
        ..super::Finder::default()
    };
    // First no folders, while every listing thread was busy; then the real answer.
    assert!(finder.rows("~/code/web/", &[]).is_empty());
    finder.folders = (Some(PathBuf::from("/x/code/web")), Vec::new());
    finder.answers += 1;
    assert_eq!(finder.rows("~/code/web/", &[])[0].title, "Use this repository");
}

#[test]
fn the_repository_s_own_checkout_comes_before_a_worktree() {
    let mut tree = checkout("/home/x/trees/web-fix", Some("https://github.com/acme/web"));
    tree.worktree = true;
    let remote = horizon_core::cloud_runtime::repository::source::parse("acme/web").unwrap();
    let mut finder = super::Finder {
        // The worktree changed last, so the search lists it first.
        checkouts: Some(vec![
            tree,
            checkout("/home/x/github/acme/web", Some("git@github.com:acme/web.git")),
        ]),
        ..super::Finder::default()
    };
    assert_eq!(
        finder.checkout_of(&remote),
        Some(PathBuf::from("/home/x/github/acme/web"))
    );
    finder.checkouts.as_mut().unwrap().pop();
    assert_eq!(
        finder.checkout_of(&remote),
        Some(PathBuf::from("/home/x/trees/web-fix")),
        "a worktree when it is the only one"
    );
}

#[test]
fn a_stopped_clone_is_not_counted_as_on_this_computer() {
    let mut stopped = checkout("/home/x/github/acme/web", Some("https://github.com/acme/web"));
    stopped.complete = false;
    stopped.stopped = true;
    let finder = super::Finder {
        checkouts: Some(vec![stopped, checkout("/home/x/code/api", None)]),
        ..super::Finder::default()
    };
    assert_eq!(finder.local_count(), 1);
}

#[test]
fn a_github_checkout_on_another_port_is_not_the_account_s_repository() {
    assert_eq!(
        super::github_name("git@github.com:Acme/Web.git").as_deref(),
        Some("acme/web")
    );
    assert_eq!(super::github_name("https://github.com:8443/acme/web"), None);
    assert_eq!(super::github_name("ssh://git@github.com:2222/acme/web"), None);
    assert_eq!(super::github_name("git://github.com:9999/acme/web"), None);
    assert_eq!(
        super::github_name("ssh://git@github.com/acme/web").as_deref(),
        Some("acme/web")
    );
    assert_eq!(
        super::checkout_title(&checkout("/x/web", Some("https://github.com:8443/acme/web"))),
        "github.com:8443/acme/web"
    );
    assert_eq!(
        super::checkout_title(&checkout("/x/tool", Some("https://git.example.com/team/tool"))),
        "git.example.com/team/tool"
    );
    let rows = rows_for(
        "",
        &[checkout(
            "/home/x/elsewhere/web",
            Some("https://github.com:8443/acme/web"),
        )],
        &names(&["acme/web"]),
    );
    assert!(
        rows.iter()
            .any(|row| matches!(&row.kind, Kind::GitHub { name } if name == "acme/web")),
        "the account's repository keeps its row"
    );
}

#[test]
fn only_part_of_a_listed_name_narrows_the_list() {
    assert!(super::narrows("ACME/we", "acme/web-board"));
    assert!(super::narrows("board", "acme/web-board"));
    assert!(
        !super::narrows("acme/docs", "acme/dev-ops-cheatsheet"),
        "a repository to look up"
    );
    assert!(!super::narrows("octocat/hello-world", "octocat-labs/hello-world-docs"));
}

#[test]
fn a_very_long_list_keeps_the_best_results_in_order() {
    let mut repositories: Vec<String> = (0..20_000).map(|n| format!("acme/service-{n:05}")).collect();
    repositories.push("acme/web".into());
    let rows = rows_for("web", &[], &repositories);
    assert_eq!(rows[0].title, "acme/web", "the best match leads");
    let rows = rows_for("", &[], &repositories);
    assert_eq!(rows.len(), super::LISTED);
    assert_eq!(rows[0].title, "acme/service-00000", "the account's order among equals");
    assert_eq!(rows[1].title, "acme/service-00001");
    // With a query that matches many, the best are what a full sort would list first.
    let mut expected: Vec<(i32, &String)> = repositories
        .iter()
        .filter_map(|name| super::fuzzy::score("sv1", name).map(|found| (found.score, name)))
        .collect();
    expected.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let expected: Vec<&str> = expected
        .iter()
        .take(super::LISTED)
        .map(|(_, name)| name.as_str())
        .collect();
    let rows = rows_for("sv1", &[], &repositories);
    let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
    assert!(expected.len() == super::LISTED && titles == expected);
}

use super::{Kind, Pick, folder_rows, is_path, names_a_host, pick, search_rows};
use crate::test_egui::DiscardTextures as _;
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
    for path in ["~", "~/code", "/srv/repo", "./here", "../there", ".", ".."] {
        assert!(is_path(path), "{path}");
    }
    for words in [
        "acme/web",
        "web",
        ".github",
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
    assert!(search_rows("main", std::slice::from_ref(&checkout), &[]).is_empty());
    assert!(search_rows("ago", std::slice::from_ref(&checkout), &[]).is_empty());
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

use super::*;
use crate::app::test_support::test_app;

fn ids(names: &[&str]) -> Vec<String> {
    names.iter().map(ToString::to_string).collect()
}

#[test]
fn everything_is_in_scope_until_something_is_chosen() {
    let scope = Scope::default();
    assert!(scope.is_all());
    assert!(scope.includes("anything"));
    assert_eq!(scope.count(), None);
}

#[test]
fn choosing_from_all_narrows_and_unchoosing_the_last_goes_back_to_all() {
    let every = ids(&["a", "b", "c"]);
    let mut scope = Scope::default();

    scope.toggle("b", &every);
    assert_eq!(scope.count(), Some(1));
    assert!(scope.includes("b") && !scope.includes("a"));

    scope.toggle("a", &every);
    assert_eq!(scope.count(), Some(2));

    scope.toggle("a", &every);
    scope.toggle("b", &every);
    assert!(scope.is_all(), "nothing chosen means everything");
}

#[test]
fn naming_every_workspace_is_the_same_as_all() {
    let every = ids(&["a", "b"]);
    let mut scope = Scope::default();
    scope.toggle("a", &every);
    scope.toggle("b", &every);
    assert!(scope.is_all());
}

#[test]
fn the_reach_follows_the_scope_and_ignores_workspaces_that_are_gone() {
    let (_temp, mut app) = test_app();
    let first = app.board.create_workspace("api");
    let second = app.board.create_workspace("docs");
    let first_local = app
        .board
        .workspace(first)
        .map(|workspace| workspace.local_id.clone())
        .expect("workspace");
    assert_eq!(app.assistant_reach(), Reach::All);

    app.assistant.scope.set_only(&first_local);
    assert_eq!(app.assistant_reach(), Reach::Only(vec![first]));
    assert_eq!(app.scope_label(), "api");

    app.assistant.scope.set_only("a workspace that was closed");
    assert_eq!(app.assistant_reach(), Reach::Only(Vec::new()));

    app.assistant.scope.set_all();
    assert_eq!(app.scope_label(), "All workspaces");
    assert!(app.board.workspace(second).is_some());
}

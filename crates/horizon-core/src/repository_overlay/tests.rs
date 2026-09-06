use super::*;
use crate::cloud_run::GitCommitSha;

fn source() -> GitSource {
    GitSource {
        repository: "synthetic/project".into(),
        commit: GitCommitSha::parse("a".repeat(40)).expect("commit"),
        branch: None,
    }
}

fn file(path: &str, marker: char, bytes: u64, executable: bool) -> OverlayChange {
    OverlayChange::new(
        path.into(),
        OverlayContent::File {
            sha256: ArtifactDigest::parse_sha256(marker.to_string().repeat(64)).expect("digest"),
            bytes,
            executable,
        },
    )
    .expect("file")
}

fn remove(path: &str) -> OverlayChange {
    OverlayChange::new(path.into(), OverlayContent::Remove).expect("removal")
}

fn link(path: &str, target: &str) -> Result<OverlayChange, OverlayPlanError> {
    OverlayChange::new(path.into(), OverlayContent::Symlink { target: target.into() })
}

#[test]
fn layers_preserve_staged_unstaged_untracked_rename_modes_and_link_intent() {
    let staged = file("renamed/script", 'b', 10, true);
    let unstaged = file("renamed/script", 'c', 13, false);
    let untracked = file("new directory/æøå", 'd', 5, false);
    let symlink = link("links/current", "../renamed/script").expect("link");
    let index = vec![staged.clone(), remove("original/script"), symlink.clone()];
    let work = vec![untracked.clone(), unstaged.clone()];
    let plan = RepositoryOverlayPlan::new(source(), index, work).expect("plan");
    assert_eq!(plan.source(), &source());
    assert_eq!(plan.index(), [symlink, remove("original/script"), staged]);
    assert_eq!(plan.working_tree(), [untracked, unstaged]);
    assert_eq!(plan.content_bytes(), 45);
    assert_eq!(plan.index()[0].path(), "links/current");
    assert_eq!(
        plan.index()[0].content(),
        &OverlayContent::Symlink {
            target: "../renamed/script".into()
        }
    );
}

#[test]
fn staged_lfs_pointer_and_hydrated_working_bytes_are_not_conflated() {
    let pointer = file("assets/image.bin", 'a', 131, false);
    let hydrated = file("assets/image.bin", 'b', 4096, false);
    let plan = RepositoryOverlayPlan::new(source(), [pointer.clone()], [hydrated.clone()]).expect("plan");
    assert_eq!(plan.index(), [pointer]);
    assert_eq!(plan.working_tree(), [hydrated]);
    assert_eq!(plan.content_bytes(), 4227);
}

#[test]
fn output_order_is_deterministic_without_changing_layer_meaning() {
    let changes = [file("z", 'a', 1, false), remove("b"), file("a", 'b', 2, true)];
    let first = RepositoryOverlayPlan::new(source(), changes.clone(), []).expect("first");
    let second = RepositoryOverlayPlan::new(source(), changes.into_iter().rev(), []).expect("second");
    assert_eq!(first, second);
    assert!(RepositoryOverlayPlan::new(source(), [], []).is_ok());
}

#[test]
fn malformed_and_ambiguous_paths_are_rejected_not_normalized() {
    for path in [
        "",
        ".",
        "..",
        "/outside",
        "../outside",
        "a/../b",
        "a/./b",
        "a//b",
        "a/",
        "C:/outside",
        "//host/share",
        "a\\b",
        "a:stream",
        "control\0name",
        "line\nbreak",
        "tab\tname",
        "name.",
        "name ",
        "a/.git./config",
    ] {
        assert_eq!(
            OverlayChange::new(path.into(), OverlayContent::Remove),
            Err(OverlayPlanError::InvalidPath)
        );
    }
    assert_eq!(
        OverlayChange::new("a".repeat(4097), OverlayContent::Remove),
        Err(OverlayPlanError::InvalidPath)
    );
    for path in [
        "a b/file",
        ".github/workflows/ci.yml",
        "docs/.gitignore",
        "æøå/雪",
        "a-b_c.file",
    ] {
        assert!(OverlayChange::new(path.into(), OverlayContent::Remove).is_ok());
    }
}

#[test]
fn excluded_paths_apply_to_all_layers_and_all_content_kinds() {
    for path in [
        ".git/config",
        "nested/.GiT/objects/a",
        ".env",
        "nested/.env.local",
        "config/production.env",
        "config/production.EnV",
        "nested/.ENV.LOCAL",
        ".envrc",
        ".ssh/id_rsa",
        ".aws/config",
        ".azure/state",
        ".kube/config",
        ".docker/config.json",
        ".config/tool/key",
        ".codex/auth.json",
        ".claude/state",
        ".claude.json",
        ".netrc",
        ".npmrc",
        ".pypirc",
        ".git-credentials",
        "nested/credentials.json",
        "nested/auth.json",
        "keys/id_ed25519",
        "target/build",
        "node_modules/pkg",
        ".cache/data",
        "__pycache__/module",
        ".venv/bin/python",
        "venv/bin/python",
        ".pytest_cache/state",
    ] {
        for content in [
            OverlayContent::Remove,
            OverlayContent::Symlink { target: "safe".into() },
            file("safe", 'a', 1, false).content().clone(),
        ] {
            assert_eq!(
                OverlayChange::new(path.into(), content),
                Err(OverlayPlanError::ExcludedPath)
            );
        }
    }
}

#[test]
fn links_preserve_literal_relative_targets_but_reject_escapes_and_exclusions() {
    for target in ["../data/file", "./../data//file", ".", "../", "../æøå"] {
        assert_eq!(
            link("links/current", target).expect("safe lexical link").content(),
            &OverlayContent::Symlink { target: target.into() }
        );
    }
    for target in [
        "",
        "../../outside",
        "/outside",
        "C:/outside",
        "\\host\\key",
        "../.git/config",
        "../.env",
        "../keys/id_rsa",
        "../name.",
        "line\nbreak",
    ] {
        assert_eq!(link("links/current", target), Err(OverlayPlanError::InvalidLink));
    }
    assert_eq!(link("link", "../outside"), Err(OverlayPlanError::InvalidLink));
    assert_eq!(link("link", &"a".repeat(4097)), Err(OverlayPlanError::InvalidLink));
}

#[test]
fn duplicate_paths_are_rejected_per_layer_but_not_across_distinct_layers() {
    let change = file("file", 'a', 1, false);
    assert_eq!(
        RepositoryOverlayPlan::new(source(), [change.clone(), remove("file")], []),
        Err(OverlayPlanError::DuplicatePath)
    );
    assert_eq!(
        RepositoryOverlayPlan::new(source(), [], [change.clone(), change.clone()]),
        Err(OverlayPlanError::DuplicatePath)
    );
    assert!(RepositoryOverlayPlan::new(source(), [change], [remove("file")]).is_ok());
}

#[test]
fn file_and_link_ancestors_cannot_contain_present_descendants() {
    for parent in [file("a", 'a', 1, false), link("a", "other").expect("link")] {
        let child = file("a/b/c", 'b', 1, false);
        for (index, work) in [
            (vec![parent.clone(), child.clone()], vec![]),
            (vec![], vec![parent.clone(), child.clone()]),
            (vec![parent.clone()], vec![child.clone()]),
            (vec![child.clone()], vec![parent.clone()]),
        ] {
            assert_eq!(
                RepositoryOverlayPlan::new(source(), index, work),
                Err(OverlayPlanError::ParentCollision)
            );
        }
    }
}

#[test]
fn removals_allow_file_directory_and_link_replacements_without_recursive_deletion() {
    let child = file("a/b", 'a', 1, false);
    let parent = file("a", 'b', 1, true);
    assert!(RepositoryOverlayPlan::new(source(), [remove("a"), child.clone()], []).is_ok());
    assert!(RepositoryOverlayPlan::new(source(), [parent.clone(), remove("a/b")], []).is_ok());
    assert!(RepositoryOverlayPlan::new(source(), [parent.clone()], [remove("a"), child.clone()]).is_ok());
    assert!(RepositoryOverlayPlan::new(source(), [child], [remove("a/b"), parent]).is_ok());
    assert!(
        RepositoryOverlayPlan::new(
            source(),
            [link("a", "other").expect("link")],
            [remove("a"), file("a/b", 'a', 1, false)]
        )
        .is_ok()
    );
}

#[test]
fn invalid_source_never_enters_a_plan_and_diagnostics_are_redacted() {
    let mut invalid = source();
    invalid.repository = "https://synthetic-user:synthetic-secret@example.invalid/private".into();
    let error = RepositoryOverlayPlan::new(invalid, [], []).expect_err("credential-bearing source");
    assert_eq!(error, OverlayPlanError::InvalidSource);
    assert!(!format!("{error:?} {error}").contains("synthetic-secret"));
    let change = link("private-name", "private-target").expect("link");
    let plan = RepositoryOverlayPlan::new(source(), [change.clone()], []).expect("plan");
    let rendered = format!("{plan:?} {change:?} {:?}", change.content());
    for private in ["synthetic/project", "private-name", "private-target"] {
        assert!(!rendered.contains(private));
    }
}

#[test]
fn count_limit_is_shared_between_layers_and_stops_infinite_input() {
    let index = (0..MAX_CHANGES).map(|number| remove(&format!("file-{number}")));
    assert!(RepositoryOverlayPlan::new(source(), index, []).is_ok());
    let index = (0..MAX_CHANGES).map(|number| remove(&format!("file-{number}")));
    assert_eq!(
        RepositoryOverlayPlan::new(source(), index, [remove("extra")]),
        Err(OverlayPlanError::ChangeLimit)
    );
    let mut consumed = 0;
    let infinite = std::iter::from_fn(|| {
        consumed += 1;
        Some(remove(&format!("file-{consumed}")))
    });
    assert_eq!(
        RepositoryOverlayPlan::new(source(), infinite, []),
        Err(OverlayPlanError::ChangeLimit)
    );
    assert_eq!(consumed, MAX_CHANGES + 1);
}

#[test]
fn metadata_and_content_budgets_fail_closed_without_overflow_or_partial_plan() {
    let long = "a".repeat(4000);
    let metadata = (0..MAX_CHANGES).map(|number| remove(&format!("{long}/{number}")));
    assert_eq!(
        RepositoryOverlayPlan::new(source(), metadata, []),
        Err(OverlayPlanError::MetadataLimit)
    );
    let full = file("file", 'a', MAX_CONTENT_BYTES, false);
    assert!(RepositoryOverlayPlan::new(source(), [full.clone()], []).is_ok());
    assert_eq!(
        RepositoryOverlayPlan::new(source(), [full], [file("file", 'b', 1, false)]),
        Err(OverlayPlanError::ContentLimit)
    );
    assert_eq!(
        OverlayChange::new(
            "file".into(),
            OverlayContent::File {
                sha256: ArtifactDigest::parse_sha256("a".repeat(64)).expect("digest"),
                bytes: u64::MAX,
                executable: false,
            }
        ),
        Err(OverlayPlanError::ContentLimit)
    );
}

use super::*;
use crate::repository_git::tests::{request, roots};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

fn tree(path: &str) -> Vec<u8> {
    format!(
        "100644 blob {}\t.gitmodules\0\
             160000 commit {}\t{path}\0",
        "a".repeat(40),
        "b".repeat(40)
    )
    .into_bytes()
}

fn config(path: &str, url: &str) -> Vec<u8> {
    format!("submodule.child.path\n{path}\0submodule.child.url\n{url}\0").into_bytes()
}

#[test]
fn exact_mapping_and_git_relative_url_resolution() {
    for url in [
        "https://github.com/owner/child.git",
        "../child.git",
        "../../owner/child.git",
    ] {
        let entries = decode(&tree("nested/child"), &config("nested/child", url), "owner/parent").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].repository, "owner/child");
        assert_eq!(entries[0].commit.as_str(), "b".repeat(40));
    }
    let mut input = config("nested/child", "../child.git");
    input.extend_from_slice(b"submodule.child.branch\nmoving-tip\0submodule.child.shallow\ntrue\0");
    assert_eq!(
        decode(&tree("nested/child"), &input, "owner/parent").unwrap()[0]
            .commit
            .as_str(),
        "b".repeat(40)
    );
}

#[test]
fn foreign_credentials_encoded_paths_and_remote_helpers_refused() {
    for url in [
        "http://github.com/o/r.git",
        "ssh://github.com/o/r.git",
        "git@github.com:o/r.git",
        "file:///tmp/repo",
        "ext::sh evil",
        "https://other.invalid/o/r.git",
        "https://github.com.evil/o/r.git",
        "https://github.com:443/o/r.git",
        "https://token@github.com/o/r.git",
        "https://github.com/o/r.git?x",
        "https://github.com/o/r.git#x",
        "https://github.com/o/%2er.git",
        "https://github.com/o/r.git/",
        "https://github.com/o//r.git",
        "../../../o/r.git",
        "../r.git\n",
        "../r.git\\x",
        "../r.git?x",
    ] {
        assert!(repository(url, "owner/parent").is_err(), "{url}");
    }
}

#[test]
fn metadata_requires_one_to_one_mapping_and_no_custom_behavior() {
    let base = config("child", "../child.git");
    for extra in [
        "include.path\n/secret\0",
        "includeif.onbranch:main.path\n/secret\0",
        "submodule.child.update\n!touch sentinel\0",
        "submodule.child.update\nmerge\0",
        "submodule.child.ignore\nall\0",
        "submodule.child.path\nchild\0",
        "submodule.child.fetchrecursesubmodules\ncustom\0",
        "submodule.child.unknown\nx\0",
        "submodule.other.path\nchild\0submodule.other.url\n../child.git\0",
    ] {
        let mut bytes = base.clone();
        bytes.extend_from_slice(extra.as_bytes());
        assert!(decode(&tree("child"), &bytes, "o/parent").is_err(), "{extra}");
    }
    assert!(decode(&tree("other"), &base, "o/parent").is_err());
    assert!(decode(&tree("child"), &base[..base.len() - 1], "o/parent").is_err());
    for mode in ["120000", "160000"] {
        let bad = String::from_utf8(tree("child")).unwrap().replacen("100644", mode, 1);
        assert!(decode(bad.as_bytes(), &base, "o/parent").is_err());
    }
    let zero = String::from_utf8(tree("child"))
        .unwrap()
        .replace(&"b".repeat(40), &"0".repeat(40));
    assert!(decode(zero.as_bytes(), &base, "o/parent").is_err());
}

#[test]
fn path_validation_rejects_redirection_and_overlapping_siblings() {
    for path in [
        "",
        "/absolute",
        "../escape",
        "a/../b",
        "a//b",
        "a/",
        "a\\b",
        "a\nb",
        ".git",
        "a/.GiT/b",
        "a/.git./b",
        "a/.git /b",
    ] {
        assert!(!valid_path(path), "{path}");
    }
    assert!(!valid_path(&"a/".repeat(17)));
    let mut both = tree("child");
    both.extend_from_slice(format!("160000 commit {}\tchild/nested\0", "c".repeat(40)).as_bytes());
    let mut cfg = config("child", "../child.git");
    cfg.extend_from_slice(b"submodule.nested.path\nchild/nested\0submodule.nested.url\n../nested.git\0");
    assert!(decode(&both, &cfg, "owner/parent").is_err());
}

#[test]
fn aggregate_metadata_repository_count_and_depth_are_not_reset() {
    let request = request();
    let mut calls = 0;
    let mut run = |args: &[&str], _: &[u8], _: bool| {
        calls += 1;
        Ok(match args[0] {
            "ls-tree" => tree("child"),
            "cat-file" => b"100\n".to_vec(),
            "config" => config("child", "../child.git"),
            _ => panic!("unexpected planning command"),
        })
    };
    let mut budget = Budget::default();
    for _ in 0..MAX_REPOSITORIES {
        read_plan(&mut run, &request, true, 0, &mut budget).unwrap();
    }
    assert!(read_plan(&mut run, &request, true, 0, &mut budget).is_err());
    assert!(read_plan(&mut run, &request, true, MAX_DEPTH, &mut Budget::default()).is_err());
    assert_eq!(calls, 3 * (MAX_REPOSITORIES + 1));
    let mut budget = Budget::default();
    for _ in 0..MAX_METADATA / MAX_RESPONSE {
        budget.charge(MAX_RESPONSE).unwrap();
    }
    assert_eq!(budget.charge(1), Err(Error::UnsupportedRepository));
    assert_eq!(
        Budget::default().charge(MAX_RESPONSE + 1),
        Err(Error::UnsupportedRepository)
    );
}

#[test]
fn child_directory_admission_refuses_nonempty_links_and_path_replacement() {
    for fault in ["nonempty", "symlink", "ancestor", "permissions", "gitfile"] {
        let root = roots();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("child")).unwrap();
        fs::set_permissions(root.path().join("child"), fs::Permissions::from_mode(0o700)).unwrap();
        match fault {
            "nonempty" => fs::write(root.path().join("child/retained"), b"keep").unwrap(),
            "gitfile" => fs::write(root.path().join("child/.git"), b"gitdir: /outside").unwrap(),
            "symlink" | "ancestor" => {
                fs::remove_dir(root.path().join("child")).unwrap();
                symlink(outside.path(), root.path().join("child")).unwrap();
            }
            _ => fs::set_permissions(root.path().join("child"), fs::Permissions::from_mode(0o777)).unwrap(),
        }
        assert!(Directory::submodule(root.path(), if fault == "ancestor" { "child/nested" } else { "child" }).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }
    let root = roots();
    let held = Directory::submodule(root.path(), "nested/child").unwrap();
    let child = held.last().unwrap();
    let git = child.fresh_git_directory().unwrap();
    assert!(child.fresh_git_directory().is_err());
    fs::rename(&child.path, root.path().join("retained")).unwrap();
    fs::create_dir(&child.path).unwrap();
    assert!(child.verify().is_err());
    assert!(git.verify().is_err());
    assert!(root.path().join("retained/.git").is_dir());
}

use super::super::bundle::{checkout_names, known};
use super::*;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

fn git(folder: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(folder)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A place to run [`CLONE`] without root: its fixed paths under `root`, no system
/// settings and no change of owner.
struct Place {
    root: tempfile::TempDir,
}

impl Place {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("workspace")).unwrap();
        std::fs::create_dir_all(root.path().join("home")).unwrap();
        Self { root }
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.root.path().join(name)
    }

    /// Runs the script with `bundle`, when there is one, and the names, as the proof does.
    fn clone(&self, bundle: Option<&Path>, names: &str, head: &str, branch: &str) -> std::process::Output {
        let _ = std::fs::remove_file(self.path("src.bundle"));
        if let Some(bundle) = bundle {
            std::fs::copy(bundle, self.path("src.bundle")).unwrap();
        }
        std::fs::write(self.path("names"), names).unwrap();
        let root = self.root.path().display().to_string();
        let script = CLONE
            .replace("git config --system --add safe.directory '*'; ", "")
            .replace("chown -R 10001:10001 /workspace/checkout", "true")
            .replace("HOME=/tmp/horizon-clone", &format!("HOME={root}/home"))
            .replace("/workspace", &format!("{root}/workspace"))
            .replace("/cache", &format!("{root}/cache"))
            .replace("/src.bundle", &format!("{root}/src.bundle"))
            .replace("/names", &format!("{root}/names"));
        Command::new("sh")
            .args(["-c", &script])
            .env("HEAD_SHA", head)
            .env("HEAD_BRANCH", branch)
            .output()
            .unwrap()
    }

    fn checkout(&self) -> std::path::PathBuf {
        self.path("workspace/checkout")
    }

    /// What the cache keeps, as the proof reads it before it bundles.
    fn kept(&self) -> Vec<String> {
        let script = KEPT.replace("/cache", &self.path("cache").display().to_string());
        let output = Command::new("sh").args(["-c", &script]).output().unwrap();
        objects(&String::from_utf8_lossy(&output.stdout))
    }
}

/// Bundles `source` at its HEAD as the proof does: a borrowing repository with the
/// branch, the tags and the head's own ref, leaving out what `kept` reaches. None when
/// nothing is new.
fn bundle(work: &Path, source: &Path, kept: &[String], branch: Option<&str>) -> (Option<std::path::PathBuf>, String) {
    let head = git(source, &["rev-parse", "HEAD"]);
    let tags = git(
        source,
        &[
            "for-each-ref",
            "--merged",
            &head,
            "--format=%(objectname) %(refname)",
            "refs/tags",
        ],
    );
    let borrowed = tempfile::tempdir_in(work).unwrap().keep();
    git(&borrowed, &["init", "-q", "--bare", "--template=", "."]);
    std::fs::write(
        borrowed.join("objects/info/alternates"),
        format!("{}\n", source.join(".git/objects").display()),
    )
    .unwrap();
    let mut names = String::new();
    if let Some(branch) = branch {
        let _ = writeln!(names, "update refs/heads/{branch} {head}");
    }
    for (object, name) in tags.lines().filter_map(|line| line.split_once(' ')) {
        let _ = writeln!(names, "update {name} {object}");
    }
    let _ = writeln!(names, "update refs/horizon/head {head}");
    let mut update = Command::new("git");
    update
        .current_dir(&borrowed)
        .args(["update-ref", "--stdin"])
        .stdin(std::process::Stdio::piped());
    let mut child = update.spawn().unwrap();
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), names.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
    let kept = known(&borrowed, kept).unwrap();
    let file = borrowed.join("head.bundle");
    let mut create = Command::new("git");
    create
        .current_dir(&borrowed)
        .args(["bundle", "create", "--quiet"])
        .arg(&file)
        .arg("--all");
    if !kept.is_empty() {
        create.arg("--not").args(&kept);
    }
    let output = create.output().unwrap();
    let bundle = if output.status.success() {
        Some(file)
    } else {
        assert!(String::from_utf8_lossy(&output.stderr).contains("empty bundle"));
        None
    };
    (bundle, checkout_names(&head, branch, &tags))
}

fn commit(source: &Path, message: &str) -> String {
    std::fs::write(source.join("file"), message).unwrap();
    git(source, &["add", "file"]);
    git(source, &["commit", "-q", "-m", message]);
    git(source, &["rev-parse", "HEAD"])
}

#[test]
fn later_proofs_send_only_what_the_cache_does_not_hold() {
    let work = tempfile::tempdir().unwrap();
    let source = work.path().join("source");
    std::fs::create_dir(&source).unwrap();
    git(&source, &["init", "-q", "-b", "main"]);
    commit(&source, "one");
    git(&source, &["tag", "-a", "v1", "-m", "first"]);
    let place = Place::new();
    assert!(place.kept().is_empty(), "a new run has no cache");

    // The first proof sends the whole history.
    let head = git(&source, &["rev-parse", "HEAD"]);
    let (first, names) = bundle(work.path(), &source, &place.kept(), Some("main"));
    let first = first.expect("a first proof sends a bundle");
    let output = place.clone(Some(&first), &names, &head, "main");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(git(&place.checkout(), &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&place.checkout(), &["describe", "--tags"]), "v1");
    assert_eq!(git(&place.checkout(), &["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(git(&place.checkout(), &["rev-parse", "origin/main"]), head);
    assert!(
        git(&place.checkout(), &["for-each-ref", "refs/kept", "refs/horizon"]).is_empty(),
        "the cache's own names stay out of the checkout"
    );

    // A new commit: the bundle holds it and leaves out what the cache keeps.
    let second = commit(&source, "two");
    let kept = place.kept();
    assert!(kept.contains(&head));
    let (next, names) = bundle(work.path(), &source, &kept, Some("main"));
    let next = next.expect("a new commit is sent");
    // The bundle's header names what it needs as `-<object>` lines before a blank one.
    let header = std::fs::read(&next).unwrap();
    let header =
        String::from_utf8_lossy(&header[..header.windows(2).position(|pair| pair == b"\n\n").unwrap()]).into_owned();
    assert!(
        header.lines().any(|line| line.starts_with(&format!("-{head}"))),
        "the bundle needs the commit the cache has: {header}"
    );
    assert!(std::fs::metadata(&next).unwrap().len() < std::fs::metadata(&first).unwrap().len());
    let output = place.clone(Some(&next), &names, &second, "main");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(git(&place.checkout(), &["rev-parse", "HEAD"]), second);
    assert_eq!(
        git(&place.checkout(), &["describe", "--tags"]).split('-').next(),
        Some("v1")
    );

    // The same commit again: nothing is sent, and the checkout is the same.
    let (none, names) = bundle(work.path(), &source, &place.kept(), Some("main"));
    assert!(none.is_none(), "nothing new, no bundle");
    let output = place.clone(None, &names, &second, "main");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(git(&place.checkout(), &["rev-parse", "HEAD"]), second);
    assert_eq!(git(&place.checkout(), &["status", "--porcelain"]), "");
}

#[test]
fn a_detached_head_and_a_new_tag_on_a_kept_commit_reach_the_checkout() {
    let work = tempfile::tempdir().unwrap();
    let source = work.path().join("source");
    std::fs::create_dir(&source).unwrap();
    git(&source, &["init", "-q", "-b", "main"]);
    let head = commit(&source, "one");
    git(&source, &["checkout", "-q", "--detach"]);
    let place = Place::new();
    let (first, names) = bundle(work.path(), &source, &[], None);
    let output = place.clone(first.as_deref(), &names, &head, "");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(git(&place.checkout(), &["rev-parse", "HEAD"]), head);
    git(&source, &["tag", "-a", "v2", "-m", "second"]);
    let (next, names) = bundle(work.path(), &source, &place.kept(), None);
    assert!(next.is_some(), "the tag object is new");
    let output = place.clone(next.as_deref(), &names, &head, "");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(git(&place.checkout(), &["describe"]), "v2");
}

#[test]
fn a_tag_deleted_between_proofs_stays_gone() {
    let work = tempfile::tempdir().unwrap();
    let source = work.path().join("source");
    std::fs::create_dir(&source).unwrap();
    git(&source, &["init", "-q", "-b", "main"]);
    let head = commit(&source, "one");
    git(&source, &["tag", "-a", "v1", "-m", "first"]);
    let place = Place::new();
    let (first, names) = bundle(work.path(), &source, &[], Some("main"));
    let output = place.clone(first.as_deref(), &names, &head, "main");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(git(&place.checkout(), &["tag", "--list"]), "v1");
    // The agent deletes the tag and proves the same commit again.
    git(&source, &["tag", "-d", "v1"]);
    let (again, names) = bundle(work.path(), &source, &place.kept(), Some("main"));
    let output = place.clone(again.as_deref(), &names, &head, "main");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        git(&place.checkout(), &["tag", "--list"]),
        "",
        "the cache gives back no tag of its own"
    );
    assert_eq!(git(&place.checkout(), &["rev-parse", "HEAD"]), head);
}

#[test]
fn fetch_head_names_the_tested_commit_after_an_incremental_proof() {
    let work = tempfile::tempdir().unwrap();
    let source = work.path().join("source");
    std::fs::create_dir(&source).unwrap();
    git(&source, &["init", "-q", "-b", "main"]);
    let place = Place::new();
    let mut kept = Vec::new();
    // Prove commits until the cache holds one whose name sorts before the next head, as
    // a listing of the cache's refs then puts it first.
    let head = loop {
        let head = commit(&source, &format!("change {}", kept.len()));
        let (bundle, names) = bundle(work.path(), &source, &place.kept(), Some("main"));
        let output = place.clone(bundle.as_deref(), &names, &head, "main");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        if kept.iter().any(|earlier: &String| earlier.as_str() < head.as_str()) {
            break head;
        }
        kept.push(head);
        assert!(kept.len() < 64, "a smaller name comes within a few commits");
    };
    assert_eq!(
        git(&place.checkout(), &["rev-parse", "FETCH_HEAD"]),
        head,
        "CI that diffs FETCH_HEAD sees the tested commit"
    );
}

#[test]
fn a_cache_without_the_prerequisites_fails_the_checkout() {
    let work = tempfile::tempdir().unwrap();
    let source = work.path().join("source");
    std::fs::create_dir(&source).unwrap();
    git(&source, &["init", "-q", "-b", "main"]);
    let first = commit(&source, "one");
    let second = commit(&source, "two");
    let (next, names) = bundle(work.path(), &source, &[first], Some("main"));
    // A cache that was removed or never held the first commit: the proof then sends
    // the whole history into a new cache.
    let place = Place::new();
    let output = place.clone(next.as_deref(), &names, &second, "main");
    assert!(!output.status.success());
}

#[test]
fn only_object_names_are_read_from_the_cache() {
    let sha1 = "a".repeat(40);
    let sha256 = "b".repeat(64);
    let listing = format!("{sha1}\n{sha256}\nnot an object\n{}\n", "c".repeat(41));
    assert_eq!(objects(&listing), [sha1, sha256]);
}

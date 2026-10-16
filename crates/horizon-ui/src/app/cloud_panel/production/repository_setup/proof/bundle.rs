//! The bundle the proof's checkout is made from: the tested commit and its history, with
//! the names a CI checkout has. `actions/checkout` puts a push build on its branch, and
//! tools such as `GitVersion` read that branch and the tags in the history, so the bundle
//! names the commit as its branch, when it has one without doubt, and carries the tags
//! already in its history. A commit made or a branch moved after the proof started is
//! never in it.
//!
//! A run keeps what its proofs sent in a cache beside its volume (see `checkout`), so a
//! later proof's bundle leaves out the objects the cache holds, and none is made when
//! nothing is new. The names then go to the checkout as a list, since a bundle without a
//! commit it names leaves that name out.
use std::path::{Path, PathBuf};
use std::process::Command;

/// What a proof sends for its checkout.
pub(super) struct Bundled {
    /// The objects the run's cache does not hold; none when it holds them all.
    pub(super) bundle: Option<PathBuf>,
    /// The branch the checkout puts the commit on: the branch checked out, or for a
    /// detached checkout the one branch at the head; none when that is not one.
    pub(super) branch: Option<String>,
    /// The checkout's names, a line of object and ref name each: the branch as origin's
    /// and the tags in the head's history.
    pub(super) names: PathBuf,
}

impl super::Job {
    /// Makes the bundle in `folder` from a private repository that borrows this one's
    /// objects and names only `head`, its branch and the tags in its history, leaving out
    /// what the objects in `kept` reach, and writes the checkout's names beside it.
    pub(super) fn bundle(
        &self,
        folder: &Path,
        head: &str,
        kept: &[String],
        log: &mut Option<std::fs::File>,
    ) -> Result<Bundled, String> {
        let git = |args: &[&str]| {
            let mut command = super::super::commit::git_command(&self.repository);
            command.args(args);
            self.capture(command, super::STEP_TIMEOUT)
                .ok()
                .map(|output| String::from_utf8_lossy(&output).trim().to_owned())
                .filter(|output| !output.is_empty())
        };
        let borrowed = folder.join("borrowed.git");
        let mut init = Command::new("git");
        init.args(["init", "--bare", "--quiet", "--template="]).arg(&borrowed);
        self.stream(init, log)?;
        let objects = git(&["rev-parse", "--git-path", "objects"]).ok_or("cannot find the repository's objects")?;
        let objects = self.repository.join(objects);
        let alternates = borrowed.join("objects").join("info");
        std::fs::create_dir_all(&alternates)
            .and_then(|()| std::fs::write(alternates.join("alternates"), format!("{}\n", objects.display())))
            .map_err(|error| format!("cannot make the bundle: {error}"))?;
        // The branch the setup is on; for a detached checkout, the branch every local or
        // origin branch at the commit names, when that is a single one.
        let branch = git(&["symbolic-ref", "--short", "-q", "HEAD"])
            .or_else(|| {
                let at_head = git(&[
                    "for-each-ref",
                    "--points-at",
                    head,
                    "--format=%(refname)",
                    "refs/heads",
                    "refs/remotes/origin",
                ])?;
                let mut names: Vec<&str> = at_head
                    .lines()
                    .filter_map(|name| {
                        name.strip_prefix("refs/heads/")
                            .or_else(|| name.strip_prefix("refs/remotes/origin/"))
                    })
                    .filter(|name| *name != "HEAD")
                    .collect();
                names.sort_unstable();
                names.dedup();
                match names.as_slice() {
                    [name] => Some((*name).to_owned()),
                    _ => None,
                }
            })
            .filter(|name| git(&["check-ref-format", "--branch", name]).is_some());
        let tags = git(&[
            "for-each-ref",
            "--merged",
            head,
            "--format=%(objectname) %(refname)",
            "refs/tags",
        ])
        .unwrap_or_default();
        name_head(&borrowed, head, branch.as_deref(), &tags)?;
        // The cache takes the bundle's refs only; a detached head is otherwise just HEAD.
        let mut keep = super::super::commit::git_command(&borrowed);
        keep.args(["update-ref", "refs/horizon/head", head]);
        self.capture(keep, super::STEP_TIMEOUT)
            .map_err(|error| format!("cannot make the bundle: {error}"))?;
        let names = folder.join("names");
        std::fs::write(&names, checkout_names(head, branch.as_deref(), &tags))
            .map_err(|error| format!("cannot make the bundle: {error}"))?;
        let bundle = folder.join("head.bundle");
        let mut create = super::super::commit::git_command(&borrowed);
        // In English whatever the locale, so an empty bundle is known by its message.
        create
            .env("LC_ALL", "C")
            .args(["bundle", "create", "--quiet"])
            .arg(&bundle)
            .arg("--all");
        let kept = known(&borrowed, kept)?;
        if !kept.is_empty() {
            create.arg("--not").args(&kept);
        }
        let bundle = match self.capture(create, super::STEP_TIMEOUT) {
            Ok(_) => Some(bundle),
            // The cache holds every object: the checkout comes from it alone.
            Err(error) if !kept.is_empty() && error.contains("empty bundle") => None,
            Err(error) => return Err(format!("cannot make the bundle: {error}")),
        };
        Ok(Bundled { bundle, branch, names })
    }
}

/// The objects of `kept` that the repository at `borrowed` has; `git bundle` refuses one
/// it does not know, such as a commit that an amend or a rebase left behind here.
pub(super) fn known(borrowed: &Path, kept: &[String]) -> Result<Vec<String>, String> {
    if kept.is_empty() {
        return Ok(Vec::new());
    }
    let mut check = super::super::commit::git_command(borrowed);
    check.args(["cat-file", "--batch-check=%(objectname)"]);
    let input = kept.join("\n") + "\n";
    let output = super::super::bounded::output(check, Some(input.into_bytes()), super::super::bounded::GIT)?;
    if !output.status.success() {
        return Err(format!(
            "cannot make the bundle: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    // A missing object prints `<name> missing`; a known one prints its name alone.
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.contains(' ') && !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// The checkout's names: `head` as origin's `branch`, when it has one, and the tags in
/// its history (`tags`, lines of object and ref name), as a fetch of the bundle names them.
pub(super) fn checkout_names(head: &str, branch: Option<&str>, tags: &str) -> String {
    use std::fmt::Write as _;
    let mut names = String::new();
    if let Some(branch) = branch {
        let _ = writeln!(names, "{head} refs/remotes/origin/{branch}");
    }
    for (object, name) in tags.lines().filter_map(|line| line.split_once(' ')) {
        let _ = writeln!(names, "{object} {name}");
    }
    names
}

/// Names `head` in the private repository at `borrowed`: its branch, if it has one, and
/// the tags in its history (`tags`, lines of object and ref name), then HEAD, as the
/// branch or detached at `head`. HEAD is set apart from the others: a new repository's
/// HEAD already names a branch, and git refuses one change of both HEAD and that branch.
fn name_head(borrowed: &Path, head: &str, branch: Option<&str>, tags: &str) -> Result<(), String> {
    use std::fmt::Write as _;
    let mut names = String::new();
    if let Some(branch) = branch {
        let _ = writeln!(names, "update refs/heads/{branch} {head}");
    }
    for (object, name) in tags.lines().filter_map(|line| line.split_once(' ')) {
        let _ = writeln!(names, "update {name} {object}");
    }
    let run = |args: &[&str], input: Option<Vec<u8>>| {
        let mut command = super::super::commit::git_command(borrowed);
        command.args(args);
        let output = super::super::bounded::output(command, input, super::super::bounded::GIT)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "cannot name the tested commit for the bundle: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    };
    run(&["update-ref", "--no-deref", "--stdin"], Some(names.into_bytes()))?;
    match branch {
        Some(branch) => run(&["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")], None),
        None => run(&["update-ref", "--no-deref", "HEAD", head], None),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn the_head_is_named_on_master_main_or_detached() {
        let work = tempfile::tempdir().unwrap();
        let git = |folder: &Path, args: &[&str]| {
            let output = Command::new("git").current_dir(folder).args(args).output().unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        let source = work.path().join("source");
        std::fs::create_dir(&source).unwrap();
        git(&source, &["init", "-q", "-b", "master"]);
        git(
            &source,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "one",
            ],
        );
        git(&source, &["tag", "v1"]);
        let head = git(&source, &["rev-parse", "HEAD"]);
        let tags = git(
            &source,
            &["for-each-ref", "--format=%(objectname) %(refname)", "refs/tags"],
        );
        for (number, branch) in [Some("master"), Some("main"), None].into_iter().enumerate() {
            let borrowed = work.path().join(format!("borrowed{number}.git"));
            // A new repository's HEAD names master or main, whatever the branch is.
            git(
                work.path(),
                &["init", "-q", "--bare", "--template=", borrowed.to_str().unwrap()],
            );
            std::fs::write(
                borrowed.join("objects/info/alternates"),
                format!("{}\n", source.join(".git/objects").display()),
            )
            .unwrap();
            name_head(&borrowed, &head, branch, &tags).unwrap();
            assert_eq!(git(&borrowed, &["rev-parse", "HEAD"]), head);
            assert_eq!(git(&borrowed, &["rev-parse", "v1"]), head);
            let named = Command::new("git")
                .current_dir(&borrowed)
                .args(["symbolic-ref", "-q", "--short", "HEAD"])
                .output()
                .unwrap();
            assert_eq!(
                String::from_utf8_lossy(&named.stdout).trim(),
                branch.unwrap_or_default(),
                "HEAD names the branch, or none when detached"
            );
        }
    }
}

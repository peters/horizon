//! The bundle the proof's checkout is made from: the tested commit and its history, with
//! the names a CI checkout has. `actions/checkout` puts a push build on its branch, and
//! tools such as `GitVersion` read that branch and the tags in the history, so the bundle
//! names the commit as its branch and carries the tags already in its history. A commit
//! made or a branch moved after the proof started is never in it.
use std::path::{Path, PathBuf};
use std::process::Command;

impl super::Job {
    /// Makes the bundle in `folder` from a private repository that borrows this one's
    /// objects and names only `head`, its branch and the tags in its history. Returns the
    /// bundle and the branch the checkout puts the commit on.
    pub(super) fn bundle(
        &self,
        folder: &Path,
        head: &str,
        log: &mut Option<std::fs::File>,
    ) -> Result<(PathBuf, String), String> {
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
        // The branch the setup is on, or the default branch a detached checkout came from.
        let branch = git(&["symbolic-ref", "--short", "-q", "HEAD"])
            .or_else(|| {
                git(&["symbolic-ref", "--short", "-q", "refs/remotes/origin/HEAD"])
                    .and_then(|remote| remote.split_once('/').map(|(_, name)| name.to_owned()))
            })
            .filter(|name| git(&["check-ref-format", "--branch", name]).is_some())
            .unwrap_or_else(|| "main".to_owned());
        let mut names = format!("update HEAD {head}\nupdate refs/heads/{branch} {head}\n");
        let tags = git(&[
            "for-each-ref",
            "--merged",
            head,
            "--format=%(objectname) %(refname)",
            "refs/tags",
        ])
        .unwrap_or_default();
        for (object, name) in tags.lines().filter_map(|line| line.split_once(' ')) {
            use std::fmt::Write as _;
            let _ = writeln!(names, "update {name} {object}");
        }
        let mut update = super::super::commit::git_command(&borrowed);
        update.args(["update-ref", "--no-deref", "--stdin"]);
        let updated = super::super::bounded::output(update, Some(names.into_bytes()), super::super::bounded::GIT)?;
        if !updated.status.success() {
            return Err(format!(
                "cannot name the tested commit for the bundle: {}",
                String::from_utf8_lossy(&updated.stderr).trim()
            ));
        }
        let bundle = folder.join("head.bundle");
        let mut create = super::super::commit::git_command(&borrowed);
        create.args(["bundle", "create", "--quiet"]).arg(&bundle).arg("--all");
        self.stream(create, log)?;
        Ok((bundle, branch))
    }
}

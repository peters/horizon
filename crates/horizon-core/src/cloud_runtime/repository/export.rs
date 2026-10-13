//! `git archive` run in a private repository, so a repository's own configuration never
//! runs a program while Horizon snapshots a commit.
//!
//! `git archive` applies the working-tree conversions a checkout would, which include a
//! content filter (`filter.<driver>.smudge` or `.process`) named by a `filter` attribute.
//! The attribute can come from the committed tree, but the driver is defined in
//! configuration. A setup agent can write a repository's configuration and its
//! `info/attributes`, so running `git archive` in that repository would let the agent's
//! files run a program in Horizon's process. Instead the archive runs in a fresh, empty
//! repository that borrows the source objects through `objects/info/alternates`: it reads
//! none of the source repository's configuration or `info/attributes`, and the host and
//! user git configuration and attributes are neutralized too, so only the committed tree's
//! `.gitattributes` applies. With no driver defined, a `filter` attribute there does
//! nothing.
//!
//! `export-ignore`, `export-subst` and `eol`, which `git archive` reads from the exported
//! tree, keep their committed effect. `export-subst` placeholders that name refs (`%D`,
//! `%(describe)`) resolve too, because the source repository's refs are copied into the
//! private repository as data. A ref is an object pointer, not configuration, so copying
//! it runs no program.
use super::{Error, Result, Runner};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Exports the committed tree at `revision` of `repository` to the tar at `output`.
/// `revision` is a committed object name that the borrowed objects resolve. The caller's
/// `timeout` bounds the export itself; locating the objects, preparing the private
/// repository and naming its refs are quick.
///
/// # Errors
/// Reports a failure to prepare the private repository, locate the source objects, or run
/// the export.
pub(super) fn committed_tree(
    repository: &Path,
    revision: &str,
    output: &Path,
    runner: &Runner<'_>,
    timeout: Duration,
) -> Result<()> {
    let private = tempfile::tempdir()?;
    // An empty file stands in for the user git configuration, so no `~/.gitconfig` is
    // read whatever the platform; `GIT_CONFIG_NOSYSTEM` drops the system configuration.
    let neutral = private.path().join("empty-config");
    std::fs::write(&neutral, "")?;
    let borrowed = private.path().join("borrowed.git");
    // No template, so the repository has no sample hooks, no `info/exclude` and no
    // `info/attributes`. Neutralizing the configuration also fixes the object format at
    // the default (SHA-1) that the source, verified SHA-1 earlier, needs for its objects.
    runner.run(
        "Prepare private export repository",
        isolated(&neutral, &mut Command::new("git"))
            .args(["init", "--bare", "--quiet", "--template="])
            .arg(&borrowed),
        Duration::from_secs(30),
    )?;
    let objects = objects_dir(repository)?;
    let info = borrowed.join("objects").join("info");
    std::fs::create_dir_all(&info)?;
    std::fs::write(info.join("alternates"), format!("{}\n", objects.display()))?;
    name_refs(repository, &neutral, &borrowed, private.path(), runner);
    runner.run(
        "git archive",
        archive_command(&neutral, &borrowed).arg(output).arg(revision),
        timeout,
    )?;
    Ok(())
}

/// The absolute path of `repository`'s object store, including a submodule's or a
/// worktree's, so the private repository can borrow it. Opened with git2, as the material
/// collection just did, so a repository the run already trusts is read the same way; git2
/// reads no configured program, and this does not drop a `safe.directory` entry that the
/// host's configuration grants, as neutralizing the configuration here would.
fn objects_dir(repository: &Path) -> Result<PathBuf> {
    let repository =
        git2::Repository::open(repository).map_err(|_| Error::Invalid("Cannot locate repository objects"))?;
    // Absolute for the alternates file, but without `canonicalize`, which prepends a
    // `\\?\` extended-length prefix on Windows that the git command line can reject.
    std::path::absolute(repository.commondir().join("objects"))
        .map_err(|_| Error::Invalid("Cannot locate repository objects"))
}

/// Copies the source repository's refs into the private repository as data, so a committed
/// `export-subst` placeholder that names refs (`%D`, `%(describe)`) resolves as it would in
/// the source. Best effort: the export does not depend on it, and a commit with no
/// ref-based substitution is unaffected, so a repository that cannot list its refs is still
/// exported.
fn name_refs(repository: &Path, neutral: &Path, borrowed: &Path, scratch: &Path, runner: &Runner<'_>) {
    let _ = copy_refs(repository, neutral, borrowed, scratch, runner);
}

fn copy_refs(repository: &Path, neutral: &Path, borrowed: &Path, scratch: &Path, runner: &Runner<'_>) -> Result<()> {
    let empty = scratch.join("no-input");
    std::fs::File::create_new(&empty)?;
    // Each ref as one `<object> <ref>` line, which is the pre-1.6 `packed-refs` format
    // (no traits), so git peels an annotated tag by reading its object on demand and
    // ignores a line whose object the alternates do not hold. Writing the file rather than
    // `update-ref`, which verifies every object in one transaction, keeps a single dangling
    // ref from dropping them all. `for-each-ref` skips a malformed ref, names no path, and
    // a ref name holds no space, so the lines never need quoting. Read with the host's git
    // configuration, as the revision resolution is, so a repository the run already trusts
    // stays trusted; `for-each-ref` reports stored object names and runs no configured
    // program.
    let listing = scratch.join("packed-refs");
    runner.to_file(
        "List repository refs",
        Command::new("git")
            .arg("--no-replace-objects")
            .arg("-C")
            .arg(repository)
            .args(["for-each-ref", "--format=%(objectname) %(refname)"]),
        &empty,
        &listing,
        Duration::from_secs(60),
    )?;
    // Only a complete listing becomes the private repository's refs; a partial one from a
    // timeout or crash stays in the scratch directory, so `git archive` runs with no packed
    // refs rather than failing on a truncated file.
    std::fs::rename(&listing, borrowed.join("packed-refs"))?;
    mirror_head(repository, neutral, borrowed, runner)
}

/// Makes the private HEAD the source's, so `%D` names it as the source does. The source on
/// a branch names that branch, which the private repository's default HEAD would otherwise
/// not; a detached source HEAD becomes a detached private HEAD at the same commit, so `%D`
/// does not read the default `refs/heads/master` as the named branch.
fn mirror_head(repository: &Path, neutral: &Path, borrowed: &Path, runner: &Runner<'_>) -> Result<()> {
    let branch = runner
        .run(
            "Read the exported branch",
            Command::new("git")
                .arg("-C")
                .arg(repository)
                .args(["symbolic-ref", "-q", "HEAD"]),
            Duration::from_secs(30),
        )
        .ok()
        .map(|head| head.trim().to_owned())
        .filter(|head| !head.is_empty());
    let borrowed_head = |args: &[&str]| {
        runner.run(
            "Name the exported HEAD",
            isolated(neutral, &mut Command::new("git"))
                .arg("-C")
                .arg(borrowed)
                .args(args),
            Duration::from_secs(30),
        )
    };
    if let Some(branch) = branch {
        borrowed_head(&["symbolic-ref", "HEAD", &branch]).map(|_| ())
    } else {
        let detached = runner.run(
            "Read the exported HEAD",
            Command::new("git")
                .arg("--no-replace-objects")
                .arg("-C")
                .arg(repository)
                .args(["rev-parse", "--verify", "--end-of-options", "HEAD"]),
            Duration::from_secs(30),
        )?;
        borrowed_head(&["update-ref", "--no-deref", "HEAD", detached.trim()]).map(|_| ())
    }
}

/// `git archive` converts line endings the way a checkout on this host would. The Linux
/// worker builds the snapshot, so pin the conversion to a Linux checkout's: Windows
/// defaults (`core.autocrlf=true`, a CRLF `core.eol`) would otherwise turn committed LF
/// text into CRLF. Explicit `eol` attributes still apply. The file modes are pinned too:
/// git keeps only whether a file is executable, so the archive holds 0644 and 0755, which
/// extraction keeps (`-p`) whatever this computer's umask, and the image copies the
/// committed modes. An empty `core.attributesFile` stands in for the user attributes file,
/// so only the committed tree's `.gitattributes` decides `eol`, `export-ignore`,
/// `export-subst` and `filter`; `isolated` drops the system attributes file too. The
/// command runs against the private `borrowed` repository; the caller appends the output
/// path and revision.
fn archive_command(neutral: &Path, borrowed: &Path) -> Command {
    let mut command = Command::new("git");
    isolated(neutral, &mut command)
        .arg("--no-replace-objects")
        .args([
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.eol=lf",
            "-c",
            "tar.umask=0022",
            "-c",
        ])
        .arg(format!("core.attributesFile={}", neutral.display()))
        .arg("-C")
        .arg(borrowed)
        .args(["archive", "--format=tar", "--output"]);
    command
}

/// Neutralizes the host and user git configuration and attributes, so the command reads
/// only what Horizon sets on it and the committed tree holds. Every `GIT_*` variable the
/// command would inherit or already carries goes first: command-scope configuration
/// (`GIT_CONFIG_COUNT` with its keys and values, `GIT_CONFIG_PARAMETERS`) would otherwise
/// reach git as configuration, and others name another repository, object store or
/// configuration file. Used for the commands that run in the private repository, where no
/// `safe.directory` grant is needed; a read of the source repository keeps that grant.
fn isolated<'c>(neutral: &Path, command: &'c mut Command) -> &'c mut Command {
    let inherited: Vec<std::ffi::OsString> = std::env::vars_os()
        .map(|(name, _)| name)
        .chain(command.get_envs().map(|(name, _)| name.to_owned()))
        // Windows reads environment names without regard to case.
        .filter(|name| name.to_string_lossy().to_ascii_uppercase().starts_with("GIT_"))
        .collect();
    for name in inherited {
        command.env_remove(name);
    }
    command
        .env("GIT_CONFIG_GLOBAL", neutral)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ATTR_NOSYSTEM", "1")
}

#[cfg(test)]
mod tests {
    use super::isolated;
    use std::process::Command;

    #[test]
    fn an_isolated_command_reads_no_command_scope_configuration() {
        let scratch = tempfile::tempdir().unwrap();
        let neutral = scratch.path().join("empty-config");
        std::fs::write(&neutral, "").unwrap();
        // Configuration that reaches git through the environment, as a process that
        // started Horizon could pass it on.
        let mut command = Command::new("git");
        command
            .current_dir(scratch.path())
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "horizon.probe")
            .env("GIT_CONFIG_VALUE_0", "reached")
            .env("GIT_CONFIG_PARAMETERS", "'horizon.legacy'='reached'")
            .env("HORIZON_KEPT", "1");
        isolated(&neutral, &mut command);
        let removed: Vec<_> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        for name in [
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_KEY_0",
            "GIT_CONFIG_VALUE_0",
            "GIT_CONFIG_PARAMETERS",
        ] {
            assert!(removed.iter().any(|removed| removed == name), "{name} is dropped");
        }
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == "HORIZON_KEPT" && value.is_some()),
            "other variables stay"
        );
        // And git itself sees none of it.
        let listed = command.args(["config", "--list"]).output().unwrap();
        let listed = String::from_utf8_lossy(&listed.stdout);
        assert!(!listed.contains("horizon."), "{listed}");
    }
}

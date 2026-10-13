//! The Git checkouts under the home folder, found once in the background so a search can
//! offer them by name, with where each came from and what it was last doing.
use super::{SKIP_NAMES, home_dir};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::SystemTime;

/// How deep below the home folder a checkout is looked for: `~/github/owner/name` is three.
const DEPTH: usize = 4;

/// The most checkouts kept; a home with more is searched no further.
const LIMIT: usize = 2000;

/// The most folders read while looking, so a huge home never keeps the search busy.
const VISITS: usize = 50_000;

/// Folders that usually hold checkouts, read first so a full home reaches them.
const FIRST: [&str; 5] = ["github", "code", "src", "projects", "dev"];

/// A folder with a repository in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkout {
    pub path: PathBuf,
    /// The address of its `origin` remote, when it has one.
    pub origin: Option<String>,
    /// The branch checked out, or `None` on a detached head.
    pub branch: Option<String>,
    /// When the repository last changed: its index, or its `HEAD` without one.
    pub touched: Option<SystemTime>,
}

/// Looks for checkouts on a background thread, most recently changed first.
#[must_use]
pub fn spawn_checkouts() -> mpsc::Receiver<Vec<Checkout>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(checkouts(&home_dir()));
    });
    receiver
}

/// The checkouts below `root`, most recently changed first. A checkout's own folders are
/// not searched further, and neither are hidden or build folders.
#[must_use]
pub fn checkouts(root: &Path) -> Vec<Checkout> {
    let mut found = Vec::new();
    let mut queue = VecDeque::from([(root.to_owned(), 0)]);
    let mut seen = std::collections::HashSet::new();
    let mut visits = 0;
    while let Some((folder, depth)) = queue.pop_front() {
        visits += 1;
        if visits > VISITS || found.len() >= LIMIT {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        // A linked folder, such as `~/github` on another disk, is followed once: its real
        // place is remembered, so two links to one folder, or a loop, are read one time.
        let mut children: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| match entry.file_type() {
                Ok(kind) if kind.is_dir() => true,
                Ok(kind) if kind.is_symlink() => {
                    entry.path().is_dir()
                        && seen.insert(std::fs::canonicalize(entry.path()).unwrap_or_else(|_| entry.path()))
                }
                _ => false,
            })
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy())
                    .is_some_and(|name| !name.starts_with('.'))
            })
            .collect();
        children.sort_by_key(|path| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (depth > 0 || !FIRST.contains(&name.as_str()), path.clone())
        });
        for child in children {
            // A build folder may itself be a checkout, but is never searched further.
            let skipped = child
                .file_name()
                .is_some_and(|name| SKIP_NAMES.contains(&name.to_string_lossy().as_ref()));
            if let Some(checkout) = read(&child) {
                found.push(checkout);
            } else if depth + 1 < DEPTH && !skipped {
                queue.push_back((child, depth + 1));
            }
        }
    }
    // A checkout reached both directly and through a link shows one time.
    let mut places = std::collections::HashSet::new();
    found.retain(|checkout| {
        places.insert(std::fs::canonicalize(&checkout.path).unwrap_or_else(|_| checkout.path.clone()))
    });
    found.sort_by(|a, b| b.touched.cmp(&a.touched).then_with(|| a.path.cmp(&b.path)));
    found
}

/// The checkout in `path`, or `None` when it holds no repository. A worktree's `.git` is a
/// file that names the repository's own folder, which holds its config.
fn read(path: &Path) -> Option<Checkout> {
    let dot = path.join(".git");
    let git = if dot.is_dir() {
        dot
    } else {
        let pointer = std::fs::read_to_string(&dot).ok()?;
        let named = PathBuf::from(pointer.strip_prefix("gitdir:")?.trim());
        if named.is_absolute() { named } else { path.join(named) }
    };
    let head = std::fs::read_to_string(git.join("HEAD")).unwrap_or_default();
    let branch = head.trim().strip_prefix("ref: refs/heads/").map(str::to_owned);
    let common = std::fs::read_to_string(git.join("commondir"))
        .map(|common| git.join(common.trim()))
        .unwrap_or_else(|_| git.clone());
    let config = std::fs::read_to_string(common.join("config")).unwrap_or_default();
    let modified = |name: &str| std::fs::metadata(git.join(name)).and_then(|meta| meta.modified()).ok();
    Some(Checkout {
        path: path.to_owned(),
        origin: origin(&config),
        branch,
        touched: modified("index").or_else(|| modified("HEAD")),
    })
}

/// The `url` of `[remote "origin"]` in a Git config.
fn origin(config: &str) -> Option<String> {
    let mut inside = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line.replace(' ', "") == "[remote\"origin\"]";
        } else if inside
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            return Some(value.trim().to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{checkouts, origin};
    use std::fs;

    #[test]
    fn the_origin_url_is_read_from_its_own_section() {
        let config = "[core]\n\turl = nope\n[remote \"upstream\"]\n\turl = https://x/up.git\n\
                      [remote \"origin\"]\n\tfetch = +refs/heads/*\n\turl = git@github.com:acme/web.git\n";
        assert_eq!(origin(config).as_deref(), Some("git@github.com:acme/web.git"));
        assert_eq!(origin("[core]\n"), None);
    }

    #[test]
    fn checkouts_are_found_without_entering_them_or_hidden_folders() {
        let home = tempfile::tempdir().unwrap();
        let repo = |path: &str, head: &str| {
            let git = home.path().join(path).join(".git");
            fs::create_dir_all(&git).unwrap();
            fs::write(git.join("HEAD"), head).unwrap();
            fs::write(
                git.join("config"),
                "[remote \"origin\"]\n\turl = https://github.com/acme/web\n",
            )
            .unwrap();
        };
        repo("github/acme/web", "ref: refs/heads/main\n");
        repo("github/acme/web/vendor/inner", "ref: refs/heads/x\n");
        repo(".hidden/secret", "ref: refs/heads/main\n");
        repo("code/tool", "0123456789abcdef\n");
        let found = checkouts(home.path());
        let paths: Vec<_> = found
            .iter()
            .map(|c| c.path.strip_prefix(home.path()).unwrap().to_owned())
            .collect();
        assert_eq!(paths.len(), 2, "{paths:?}");
        let web = found.iter().find(|c| c.path.ends_with("github/acme/web")).unwrap();
        assert_eq!(web.branch.as_deref(), Some("main"));
        assert_eq!(web.origin.as_deref(), Some("https://github.com/acme/web"));
        let tool = found.iter().find(|c| c.path.ends_with("code/tool")).unwrap();
        assert_eq!(tool.branch, None, "a detached head has no branch");
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_folder_is_followed_once() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let git = elsewhere.path().join("acme/web/.git");
        fs::create_dir_all(&git).unwrap();
        fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), home.path().join("github")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), home.path().join("code")).unwrap();
        // A loop back to the home folder ends.
        std::os::unix::fs::symlink(home.path(), home.path().join("loop")).unwrap();
        let found = checkouts(home.path());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].path.ends_with("acme/web"));
    }

    #[test]
    fn a_checkout_named_like_a_build_folder_is_listed_but_build_folders_are_not_searched() {
        let home = tempfile::tempdir().unwrap();
        for repo in ["code/vendor", "code/node_modules/pkg"] {
            let git = home.path().join(repo).join(".git");
            fs::create_dir_all(&git).unwrap();
            fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        }
        let found = checkouts(home.path());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].path.ends_with("code/vendor"));
    }
}

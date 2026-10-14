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

/// Home folders that macOS guards with a privacy prompt; opening New cloud never asks.
const GUARDED: [&str; 8] = [
    "Desktop",
    "Documents",
    "Downloads",
    "Pictures",
    "Movies",
    "Music",
    "Library",
    "Public",
];

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
    /// A finished checkout of its own: its `.git` is a folder (no worktree), its `HEAD`
    /// names a commit, and no clone into it stopped halfway. Only such a checkout stands in
    /// for a clone.
    pub complete: bool,
}

/// Looks for checkouts on a background thread, most recently changed first.
#[must_use]
pub fn spawn_checkouts() -> mpsc::Receiver<Vec<Checkout>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        // What is found so far goes out every second, so one slow folder hides nothing
        // found before it; the channel closes when the search is done.
        let mut last = std::time::Instant::now();
        // A closed dialog stops the search: nobody reads it any more.
        let found = checkouts_while(&home_dir(), &mut |so_far| {
            if last.elapsed() < std::time::Duration::from_secs(1) {
                return true;
            }
            last = std::time::Instant::now();
            sender.send(settled(so_far.to_vec())).is_ok()
        });
        let _ = sender.send(found);
    });
    receiver
}

/// The checkouts below `root`, most recently changed first. A checkout's own folders are
/// not searched further, and neither are hidden or build folders.
#[must_use]
pub fn checkouts(root: &Path) -> Vec<Checkout> {
    checkouts_while(root, &mut |_| true)
}

/// As [`checkouts`], telling `progress` what was found after each folder read; it stops
/// when `progress` says so.
fn checkouts_while(root: &Path, progress: &mut dyn FnMut(&[Checkout]) -> bool) -> Vec<Checkout> {
    let mut found = Vec::new();
    // Folders below `~/github` and the other usual places are all read before any other
    // folder, so a home with very many folders still reaches them within the visit cap.
    let mut preferred: VecDeque<(PathBuf, usize)> = VecDeque::new();
    let mut queue = VecDeque::from([(root.to_owned(), 0)]);
    // The home folder itself counts as read, so a link back to it is not followed.
    let mut seen = std::collections::HashSet::from([std::fs::canonicalize(root).unwrap_or_else(|_| root.to_owned())]);
    // Where the folders that macOS guards really are, so no link leads into them either.
    let guarded: Vec<PathBuf> = if cfg!(target_os = "macos") {
        GUARDED
            .iter()
            .flat_map(|name| {
                let named = root.join(name);
                let real = std::fs::canonicalize(&named).unwrap_or_else(|_| named.clone());
                [named, real]
            })
            .collect()
    } else {
        Vec::new()
    };
    // The usual places are read first of all, before the home folder's own entries spend
    // the budget.
    for name in FIRST {
        let place = root.join(name);
        if let Some(real) = resolved(&place, &guarded)
            && real.is_dir()
            && seen.insert(real)
        {
            if let Some(checkout) = read(&place, &guarded) {
                found.push(checkout);
            } else {
                preferred.push_back((place, 1));
            }
        }
    }
    let mut visits = 0;
    while let Some((folder, depth)) = preferred.pop_front().or_else(|| queue.pop_front()) {
        visits += 1;
        if visits > VISITS || found.len() >= LIMIT {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        // Every folder's real place is remembered, so a folder reached both directly and
        // through a link, such as `~/github` on another disk, or a loop, is read one time.
        // Every entry looked at counts against the budget, so one folder with very many
        // entries cannot keep the search busy past it.
        let mut probed = 0;
        let mut children: Vec<PathBuf> = entries
            .flatten()
            .take(VISITS.saturating_sub(visits))
            .inspect(|_| probed += 1)
            // Hidden folders, and on macOS the folders it guards with a privacy prompt, are
            // passed by before a link among them is remembered.
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let guarded = depth == 0 && cfg!(target_os = "macos") && GUARDED.contains(&name.as_ref());
                !name.starts_with('.') && !guarded
            })
            .filter_map(|entry| {
                let path = entry.path();
                let kind = entry.file_type().ok()?;
                if !kind.is_dir() && !kind.is_symlink() {
                    return None;
                }
                // Links are followed one at a time and never into a guarded folder, so its
                // real place is known before anything inside it is looked at.
                let place = resolved(&path, &guarded)?;
                (place.is_dir() && seen.insert(place)).then_some(path)
            })
            .collect();
        visits += probed;
        children.sort_by_key(|path| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (depth > 0 || !FIRST.contains(&name.as_str()), path.clone())
        });
        let below_preferred = depth > 0 && preferred_root(root, &folder);
        for child in children {
            // A build folder may itself be a checkout, but is never searched further.
            let skipped = child
                .file_name()
                .is_some_and(|name| SKIP_NAMES.contains(&name.to_string_lossy().as_ref()));
            if found.len() >= LIMIT {
                break;
            }
            if let Some(checkout) = read(&child, &guarded) {
                found.push(checkout);
            } else if depth + 1 < DEPTH && !skipped {
                let first = depth == 0
                    && child
                        .file_name()
                        .is_some_and(|name| FIRST.contains(&name.to_string_lossy().as_ref()));
                if first || below_preferred {
                    preferred.push_back((child, depth + 1));
                } else {
                    queue.push_back((child, depth + 1));
                }
            }
        }
        if !progress(&found) {
            break;
        }
    }
    settled(found)
}

/// Where `path` really is, with every link in it followed one at a time: `None` when a
/// step would enter a guarded folder, a link cannot be read, or links loop. Each link is
/// read with `read_link` from a place already known to be outside the guarded folders, so
/// nothing inside them is ever touched.
fn resolved(path: &Path, guarded: &[PathBuf]) -> Option<PathBuf> {
    if guarded.is_empty() {
        return Some(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()));
    }
    let inside = |place: &Path| guarded.iter().any(|guarded| place.starts_with(guarded));
    let mut rest: Vec<std::ffi::OsString> = lexical(path).iter().map(std::ffi::OsStr::to_os_string).collect();
    rest.reverse();
    let mut done = PathBuf::new();
    let mut hops = 0;
    while let Some(part) = rest.pop() {
        let next = done.join(&part);
        if inside(&next) {
            return None;
        }
        let is_link = std::fs::symlink_metadata(&next).is_ok_and(|meta| meta.file_type().is_symlink());
        if !is_link {
            done = next;
            continue;
        }
        hops += 1;
        if hops > 32 {
            return None;
        }
        let target = std::fs::read_link(&next).ok()?;
        let target = lexical(&done.join(target));
        // The link's target replaces what was resolved so far, and the rest follows it.
        let mut parts: Vec<std::ffi::OsString> = target.iter().map(std::ffi::OsStr::to_os_string).collect();
        parts.reverse();
        rest.extend(parts);
        done = PathBuf::new();
    }
    let done = lexical(&done);
    (!inside(&done)).then_some(done)
}

/// The most bytes read of one Git metadata file.
const SMALL_FILE: u64 = 256 * 1024;

/// A small regular file's text, reached without entering a guarded folder: never a pipe or
/// a device, whose read could wait forever, and never more than [`SMALL_FILE`] bytes.
fn small_file(path: &Path, guarded: &[PathBuf]) -> Option<String> {
    use std::io::Read as _;
    let place = resolved(path, guarded)?;
    let meta = std::fs::symlink_metadata(&place).ok()?;
    if !meta.is_file() || meta.len() > SMALL_FILE {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(&place)
        .ok()?
        .take(SMALL_FILE)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// `path` with its `.` and `..` worked out from the text alone, without reading the disk:
/// a link's target is judged before anything is looked up through it.
fn lexical(path: &Path) -> PathBuf {
    let mut clean = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                clean.pop();
            }
            other => clean.push(other),
        }
    }
    clean
}

/// Whether `folder` lies below one of the usual places in `root`, such as `~/github`.
fn preferred_root(root: &Path, folder: &Path) -> bool {
    folder
        .strip_prefix(root)
        .ok()
        .and_then(|below| below.components().next())
        .is_some_and(|first| FIRST.contains(&first.as_os_str().to_string_lossy().as_ref()))
}

/// `found` with a checkout reached both directly and through a link shown one time, most
/// recently changed first.
fn settled(mut found: Vec<Checkout>) -> Vec<Checkout> {
    let mut places = std::collections::HashSet::new();
    found.retain(|checkout| {
        places.insert(std::fs::canonicalize(&checkout.path).unwrap_or_else(|_| checkout.path.clone()))
    });
    found.sort_by(|a, b| b.touched.cmp(&a.touched).then_with(|| a.path.cmp(&b.path)));
    found
}

/// The checkout in `path`, or `None` when it holds no repository. A worktree's `.git` is a
/// file that names the repository's own folder, which holds its config.
fn read(path: &Path, guarded: &[PathBuf]) -> Option<Checkout> {
    // A checkout whose repository lies in a guarded folder, also through links, is listed,
    // but nothing of it is read there.
    let unread = || Checkout {
        path: path.to_owned(),
        origin: None,
        branch: None,
        touched: None,
        complete: false,
    };
    let dot = path.join(".git");
    let kind = std::fs::symlink_metadata(&dot).ok()?.file_type();
    let Some(dot) = resolved(&dot, guarded) else {
        return Some(unread());
    };
    let meta = std::fs::symlink_metadata(&dot).ok()?;
    let own = meta.is_dir();
    let git = if own {
        dot
    } else if meta.is_file() || kind.is_symlink() {
        let pointer = small_file(&dot, guarded)?;
        let named = PathBuf::from(pointer.strip_prefix("gitdir:")?.trim());
        let named = if named.is_absolute() { named } else { path.join(named) };
        let Some(git) = resolved(&named, guarded) else {
            return Some(unread());
        };
        git
    } else {
        return None;
    };
    let head = small_file(&git.join("HEAD"), guarded).unwrap_or_default();
    let branch = head.trim().strip_prefix("ref: refs/heads/").map(str::to_owned);
    let common = small_file(&git.join("commondir"), guarded).map_or_else(
        || Some(git.clone()),
        |common| resolved(&git.join(common.trim()), guarded),
    );
    let config = common
        .and_then(|common| small_file(&common.join("config"), guarded))
        .unwrap_or_default();
    let modified = |name: &str| {
        std::fs::symlink_metadata(git.join(name))
            .and_then(|meta| meta.modified())
            .ok()
    };
    let complete = own
        && head_resolves(&git, head.trim(), guarded)
        && !crate::cloud_runtime::repository::source::clone_stopped(path);
    Some(Checkout {
        path: path.to_owned(),
        origin: origin(&config),
        branch,
        touched: modified("index").or_else(|| modified("HEAD")),
        complete,
    })
}

/// Whether `head`, the text of `HEAD` in `git`, names a commit: an object name, or a branch
/// whose ref is a file or is listed in `packed-refs`. Read from files only.
fn head_resolves(git: &Path, head: &str, guarded: &[PathBuf]) -> bool {
    let Some(name) = head.strip_prefix("ref: ") else {
        return head.len() >= 40 && head.bytes().all(|byte| byte.is_ascii_hexdigit());
    };
    let name = name.trim();
    if name.split('/').any(|part| part.is_empty() || part == "..") {
        return false;
    }
    small_file(&git.join(name), guarded).is_some_and(|value| !value.trim().is_empty())
        || packed_ref(&git.join("packed-refs"), name, guarded)
}

/// The most bytes of `packed-refs` read while looking for one ref: a repository with very
/// many tags keeps them all there.
const PACKED_REFS: u64 = 64 * 1024 * 1024;

/// Whether `packed-refs` at `path` lists the ref `name`, read line by line from a regular
/// file reached without entering a guarded folder.
fn packed_ref(path: &Path, name: &str, guarded: &[PathBuf]) -> bool {
    use std::io::{BufRead as _, Read as _};
    let Some(place) = resolved(path, guarded) else {
        return false;
    };
    if !std::fs::symlink_metadata(&place).is_ok_and(|meta| meta.is_file()) {
        return false;
    }
    let Ok(file) = std::fs::File::open(&place) else {
        return false;
    };
    std::io::BufReader::new(file.take(PACKED_REFS))
        .lines()
        .map_while(Result::ok)
        .any(|line| line.split_once(' ').is_some_and(|(_, listed)| listed.trim() == name))
}

/// The `url` of `[remote "origin"]` in a Git config.
fn origin(config: &str) -> Option<String> {
    let mut inside = false;
    for line in config.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[') {
            // The section name is case-insensitive in Git; the subsection name is not.
            let header = header.trim_end_matches(']');
            inside = header.split_once('"').is_some_and(|(section, rest)| {
                section.trim().eq_ignore_ascii_case("remote") && rest.trim_end_matches('"') == "origin"
            });
        } else if inside
            && let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case("url")
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
        assert_eq!(
            origin("[REMOTE \"origin\"]\n\tURL = https://github.com/acme/web\n").as_deref(),
            Some("https://github.com/acme/web"),
            "section and key names are case-insensitive"
        );
        assert_eq!(
            origin("[remote \"Origin\"]\n\turl = x\n"),
            None,
            "the remote's name is exact"
        );
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

    #[test]
    fn folders_below_the_usual_places_are_preferred() {
        let root = std::path::Path::new("/home/x");
        assert!(super::preferred_root(root, &root.join("github/acme")));
        assert!(super::preferred_root(root, &root.join("code")));
        assert!(!super::preferred_root(root, &root.join("Videos/github")));
    }

    #[test]
    fn a_link_target_is_worked_out_without_the_disk() {
        let joined = std::path::Path::new("/home/x/github/../../x/Documents/code");
        assert_eq!(super::lexical(joined), std::path::Path::new("/home/x/Documents/code"));
    }

    #[test]
    fn a_worktree_of_a_repository_in_a_guarded_folder_is_listed_without_reading_it() {
        let home = tempfile::tempdir().unwrap();
        let worktree = home.path().join("code/web");
        fs::create_dir_all(&worktree).unwrap();
        let gitdir = home.path().join("Documents/main/.git/worktrees/web");
        fs::write(worktree.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        let checkout = super::read(&worktree, &[home.path().join("Documents")]).unwrap();
        assert_eq!(checkout.origin, None);
        assert_eq!(checkout.branch, None);
    }

    #[cfg(unix)]
    #[test]
    fn a_chain_of_links_into_a_guarded_folder_is_stopped() {
        let home = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(home.path()).unwrap();
        let documents = home.join("Documents");
        fs::create_dir_all(documents.join("code")).unwrap();
        std::os::unix::fs::symlink(&documents, home.join("alias")).unwrap();
        std::os::unix::fs::symlink(home.join("alias"), home.join("github")).unwrap();
        let guarded = [documents.clone()];
        assert_eq!(super::resolved(&home.join("github"), &guarded), None);
        assert_eq!(super::resolved(&home.join("github/code"), &guarded), None);
        fs::create_dir_all(home.join("code")).unwrap();
        assert_eq!(super::resolved(&home.join("code"), &guarded), Some(home.join("code")));
    }

    #[cfg(unix)]
    #[test]
    fn a_git_pipe_is_never_read() {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("code/web");
        fs::create_dir_all(&repo).unwrap();
        let made = std::process::Command::new("mkfifo").arg(repo.join(".git")).status();
        if !made.is_ok_and(|status| status.success()) {
            return;
        }
        assert_eq!(super::read(&repo, &[]), None, "a pipe is not a repository");
        assert!(super::small_file(&repo.join(".git"), &[]).is_none());
    }

    #[test]
    fn the_usual_places_are_read_although_the_home_folder_is_full() {
        let home = tempfile::tempdir().unwrap();
        for n in 0..50 {
            fs::create_dir_all(home.path().join(format!("aaa-{n:03}"))).unwrap();
        }
        let git = home.path().join("github/acme/web/.git");
        fs::create_dir_all(&git).unwrap();
        fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        // Reading the usual places first, a search stopped after the first folder still has it.
        let mut reads = 0;
        let found = super::checkouts_while(home.path(), &mut |_| {
            reads += 1;
            reads < 3
        });
        assert!(
            found.iter().any(|checkout| checkout.path.ends_with("github/acme/web")),
            "{found:?}"
        );
    }

    #[test]
    fn a_checkout_is_complete_only_with_a_commit_and_no_stopped_clone() {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("code/web");
        let git = repo.join(".git");
        fs::create_dir_all(git.join("refs/heads")).unwrap();
        fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert!(!super::read(&repo, &[]).unwrap().complete, "no commit on main yet");
        fs::write(
            git.join("refs/heads/main"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();
        assert!(super::read(&repo, &[]).unwrap().complete);
        fs::remove_file(git.join("refs/heads/main")).unwrap();
        fs::write(
            git.join("packed-refs"),
            "0123456789abcdef0123456789abcdef01234567 refs/heads/main\n",
        )
        .unwrap();
        assert!(super::read(&repo, &[]).unwrap().complete, "a packed ref counts");
    }

    #[test]
    fn a_ref_in_a_large_packed_refs_is_found() {
        use std::fmt::Write as _;
        let home = tempfile::tempdir().unwrap();
        let git = home.path().join(".git");
        fs::create_dir_all(&git).unwrap();
        let mut packed = (0..8000).fold(String::new(), |mut text, n| {
            let _ = writeln!(
                text,
                "0123456789abcdef0123456789abcdef01234567 refs/tags/release-{n:05}"
            );
            text
        });
        packed.push_str("0123456789abcdef0123456789abcdef01234567 refs/heads/main\n");
        fs::write(git.join("packed-refs"), packed).unwrap();
        assert!(super::head_resolves(&git, "ref: refs/heads/main", &[]));
    }
}

//! The Git checkouts under the home folder, found once in the background so a search can
//! offer them by name, with where each came from and what it was last doing.
use super::SKIP_NAMES;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// A finished checkout: its `HEAD` names a commit, read from its repository's refs (a
    /// worktree's are its main repository's), and no clone into it stopped halfway. Only
    /// such a checkout stands in for a clone.
    pub complete: bool,
    /// A clone into it started and stopped halfway: its files are no checkout to use, and
    /// cloning the repository again picks the clone up.
    pub stopped: bool,
    /// A linked worktree (its git folder names a `commondir`), on a branch of its own,
    /// rather than the repository's own checkout, also one cloned with `--separate-git-dir`.
    pub worktree: bool,
}

/// The most searches for checkouts whose threads still run, across every dialog: a closed
/// dialog cannot stop a search held up by a folder on a mount that hangs.
const SEARCHES: usize = 2;

/// How many searches for checkouts still run.
static SEARCHING: AtomicUsize = AtomicUsize::new(0);

/// Looks for checkouts on a background thread, most recently changed first. While
/// [`SEARCHES`] searches still run, none starts: `None`, to be asked again later.
#[must_use]
pub fn spawn_checkouts() -> Option<mpsc::Receiver<Vec<Checkout>>> {
    /// Counts the search out however it ends.
    struct Running;
    impl Drop for Running {
        fn drop(&mut self) {
            SEARCHING.fetch_sub(1, Ordering::AcqRel);
        }
    }
    if SEARCHING.fetch_add(1, Ordering::AcqRel) >= SEARCHES {
        SEARCHING.fetch_sub(1, Ordering::AcqRel);
        return None;
    }
    let (sender, receiver) = mpsc::channel();
    let running = Running;
    std::thread::spawn(move || {
        let _running = running;
        // What is found so far goes out every second, so one slow folder hides nothing
        // found before it; the channel closes when the search is done.
        let mut last = std::time::Instant::now();
        // Without a home folder there is nowhere of the person's own to search: the file
        // system's root holds system and mounted folders, not their checkouts.
        let Some(home) = own_home() else {
            let _ = sender.send(Vec::new());
            return;
        };
        // A closed dialog stops the search: nobody reads it any more.
        let found = checkouts_while(&home, &mut |so_far| {
            if last.elapsed() < std::time::Duration::from_secs(1) {
                return true;
            }
            last = std::time::Instant::now();
            sender.send(settled(so_far.to_vec())).is_ok()
        });
        let _ = sender.send(found);
    });
    Some(receiver)
}

/// The person's home folder, from `HOME` or, on Windows, `USERPROFILE`; `None` without
/// either.
fn own_home() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .into_iter()
        .filter_map(std::env::var_os)
        .find(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// The checkout in `path`, read as the search reads one, or `None` when it holds no
/// repository. On macOS nothing is read in the folders macOS guards below the home folder
/// unless the path itself lies inside one, as when it is typed there: such a folder itself,
/// as when `~/` is listed, is no checkout, nor is a link into one.
#[must_use]
pub fn checkout_in(path: &Path) -> Option<Checkout> {
    let guarded = own_home().map(|home| guarded_below(&home)).unwrap_or_default();
    checkout_outside(path, &guarded)
}

/// As [`checkout_in`], with the guarded folders given.
fn checkout_outside(path: &Path, guarded: &[PathBuf]) -> Option<Checkout> {
    if guarded.is_empty() {
        return read(path, guarded);
    }
    // The system takes `..` after a link from the link's target, not from the text, so a
    // path with one could lead anywhere: it is not read as a checkout.
    if path.components().any(|part| part == std::path::Component::ParentDir) {
        return None;
    }
    let named = lexical(path);
    let inside: Vec<&PathBuf> = guarded
        .iter()
        .filter(|folder| below(&named, folder) && !below(folder, &named))
        .collect();
    // A path typed inside a guarded folder is looked at there, but no link in it leads on
    // into another one; the folder's real place is the same folder.
    let real: Vec<PathBuf> = inside
        .iter()
        .map(|folder| std::fs::canonicalize(folder).unwrap_or_else(|_| (*folder).clone()))
        .collect();
    let others: Vec<PathBuf> = guarded
        .iter()
        .filter(|folder| !inside.contains(folder) && !real.contains(folder))
        .cloned()
        .collect();
    // Links are followed one step at a time, refused before one enters a guarded folder.
    resolved(path, &others)?;
    read(path, guarded)
}

/// Where the folders that macOS guards below `root` are, by name and for real, so no link
/// leads into them either; none elsewhere.
fn guarded_below(root: &Path) -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
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
    }
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
    let guarded = guarded_below(root);
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
/// nothing inside them is ever touched. A `..` is taken one step at a time from what is
/// resolved so far, which holds no link, so it goes where the system would go.
fn resolved(path: &Path, guarded: &[PathBuf]) -> Option<PathBuf> {
    if guarded.is_empty() {
        return Some(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()));
    }
    let inside = |place: &Path| guarded.iter().any(|guarded| below(place, guarded));
    let parts = |path: &Path| -> Vec<std::ffi::OsString> {
        path.components()
            .rev()
            .map(|part| part.as_os_str().to_os_string())
            .collect()
    };
    let mut rest = parts(path);
    let mut done = PathBuf::new();
    let mut hops = 0;
    while let Some(part) = rest.pop() {
        if part == ".." {
            done.pop();
            continue;
        }
        if part == "." {
            continue;
        }
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
        // The link's target follows from the link's folder, or replaces it when absolute,
        // and the rest of the path follows the target.
        let target = std::fs::read_link(&next).ok()?;
        if target.has_root() {
            done = PathBuf::new();
        }
        rest.extend(parts(&target));
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
    // A file that grew since it was measured is still read no further than the limit, and
    // is not taken cut short.
    let mut text = String::new();
    std::fs::File::open(&place)
        .ok()?
        .take(SMALL_FILE + 1)
        .read_to_string(&mut text)
        .ok()?;
    (u64::try_from(text.len()).ok()? <= SMALL_FILE).then_some(text)
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
    let dot = path.join(".git");
    let kind = std::fs::symlink_metadata(&dot).ok()?.file_type();
    // A checkout whose repository lies in a guarded folder, also through links, is listed,
    // but nothing of it is read there.
    let unread = || Checkout {
        path: path.to_owned(),
        origin: None,
        branch: None,
        touched: None,
        complete: false,
        stopped: false,
        worktree: !kind.is_dir(),
    };
    let Some(dot) = resolved(&dot, guarded) else {
        return Some(unread());
    };
    let meta = std::fs::symlink_metadata(&dot).ok()?;
    let git = if meta.is_dir() {
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
    // Reftable's placeholder in `HEAD` names no branch; the branch is kept in its tables.
    let branch = head
        .trim()
        .strip_prefix("ref: refs/heads/")
        .filter(|name| *name != ".invalid")
        .map(str::to_owned);
    let commondir = small_file(&git.join("commondir"), guarded);
    let worktree = commondir.is_some();
    let common = commondir.map_or_else(
        || Some(git.clone()),
        |common| resolved(&git.join(common.trim()), guarded),
    );
    let config = common
        .as_ref()
        .and_then(|common| small_file(&common.join("config"), guarded))
        .unwrap_or_default();
    let modified = |name: &str| {
        std::fs::symlink_metadata(git.join(name))
            .and_then(|meta| meta.modified())
            .ok()
    };
    let stopped = small_file(
        &git.join(crate::cloud_runtime::repository::source::CLONE_MARKER),
        guarded,
    )
    .is_some_and(|marker| crate::cloud_runtime::repository::source::clone_stopped(&marker));
    // Branches live in the repository's shared folder, also for a worktree.
    let complete = common.is_some_and(|common| head_resolves(&common, head.trim(), guarded)) && !stopped;
    Some(Checkout {
        path: path.to_owned(),
        origin: origin(&config),
        branch,
        touched: modified("index").or_else(|| modified("HEAD")),
        complete,
        stopped,
        worktree,
    })
}

/// Whether `head`, the text of `HEAD` in `git`, names a commit: an object name, or a branch
/// whose ref is a file or is listed in `packed-refs`. Read from files only.
fn head_resolves(git: &Path, head: &str, guarded: &[PathBuf]) -> bool {
    let Some(name) = head.strip_prefix("ref: ") else {
        return object_name(head);
    };
    let name = name.trim();
    // A repository that keeps its refs in reftable (Git 2.45 and later) names the
    // placeholder `refs/heads/.invalid` in `HEAD`, a file that never exists: its refs are
    // in tables this does not read, so such a repository is never taken as finished.
    if !ref_name(name) {
        return false;
    }
    small_file(&git.join(name), guarded).is_some_and(|value| object_name(value.trim()))
        || packed_ref(&git.join("packed-refs"), name, guarded)
}

/// Whether `name` is a ref Git could hold below its directory: plain names joined by `/`.
/// Git refuses `\` and `:` in a ref, so a Windows separator, drive or share in `HEAD`
/// never leads out of the Git directory.
fn ref_name(name: &str) -> bool {
    !name.contains(['\\', ':'])
        && name
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && Path::new(name)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

/// Whether `text` is a Git object name: 40 or 64 hexadecimal digits.
fn object_name(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Whether `path` lies below `folder`, the names compared without regard to case, as a
/// case-insensitive file system such as macOS's usual one sees them.
fn below(path: &Path, folder: &Path) -> bool {
    let mut path = path.components();
    folder.components().all(|part| {
        path.next().is_some_and(|other| {
            other.as_os_str().to_string_lossy().to_lowercase() == part.as_os_str().to_string_lossy().to_lowercase()
        })
    })
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
    fn a_guarded_folder_itself_is_no_checkout() {
        let home = tempfile::tempdir().unwrap();
        let documents = home.path().join("Documents");
        fs::create_dir_all(documents.join(".git")).unwrap();
        let guarded = [documents.clone()];
        assert_eq!(super::checkout_outside(&documents, &guarded), None);
        let spelled = home.path().join("documents");
        assert_eq!(
            super::checkout_outside(&spelled, &guarded),
            None,
            "names compare without case"
        );
        assert!(super::checkout_outside(&documents, &[]).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_into_a_guarded_folder_is_no_checkout_but_a_path_typed_there_is() {
        let home = tempfile::tempdir().unwrap();
        let documents = home.path().join("Documents");
        let project = documents.join("project");
        fs::create_dir_all(project.join(".git")).unwrap();
        let alias = home.path().join("alias");
        std::os::unix::fs::symlink(&project, &alias).unwrap();
        let guarded = [documents.clone()];
        assert_eq!(super::checkout_outside(&alias, &guarded), None);
        let typed = super::checkout_outside(&project, &guarded).expect("typed inside");
        assert!(typed.origin.is_none(), "listed with nothing read");
        // A link inside one guarded folder does not lead into another.
        let desktop = home.path().join("Desktop");
        fs::create_dir_all(desktop.join("proj/.git")).unwrap();
        let across = documents.join("lnk");
        std::os::unix::fs::symlink(desktop.join("proj"), &across).unwrap();
        assert_eq!(super::checkout_outside(&across, &[documents.clone(), desktop]), None);
        // `alias/..` is the guarded folder for the system, whatever the text folds to.
        fs::create_dir_all(home.path().join("project/.git")).unwrap();
        assert_eq!(super::checkout_outside(&alias.join("../project"), &guarded), None);
        // Nor does a `..` inside a link's target, after a link in it, fold away the guard.
        fs::create_dir_all(documents.join("sub")).unwrap();
        fs::create_dir_all(home.path().join("a")).unwrap();
        fs::create_dir_all(home.path().join("b")).unwrap();
        std::os::unix::fs::symlink(documents.join("sub"), home.path().join("b/lnk")).unwrap();
        let hop = home.path().join("a/hop");
        std::os::unix::fs::symlink("../b/lnk/..", &hop).unwrap();
        assert_eq!(super::resolved(&hop, &guarded), None);
        assert_eq!(super::checkout_outside(&hop, &guarded), None);
    }

    #[test]
    fn a_ref_with_a_windows_separator_drive_or_dot_part_is_refused() {
        for bad in [
            "..\\outside",
            "C:\\share\\ref",
            "C:/x",
            "refs/../x",
            "refs//x",
            "./refs",
            "/etc/x",
            "refs\\heads\\main",
        ] {
            assert!(!super::ref_name(bad), "{bad}");
        }
        assert!(super::ref_name("refs/heads/feat/a.b"));
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
    fn a_worktree_on_a_branch_with_a_commit_is_complete() {
        let home = tempfile::tempdir().unwrap();
        let main = home.path().join("code/web/.git");
        let own = main.join("worktrees/fix");
        fs::create_dir_all(&own).unwrap();
        fs::write(own.join("HEAD"), "ref: refs/heads/fix\n").unwrap();
        fs::write(own.join("commondir"), "../..\n").unwrap();
        fs::write(
            main.join("config"),
            "[remote \"origin\"]\n\turl = https://github.com/acme/web\n",
        )
        .unwrap();
        let tree = home.path().join("trees/fix");
        fs::create_dir_all(&tree).unwrap();
        fs::write(tree.join(".git"), format!("gitdir: {}\n", own.display())).unwrap();
        assert!(
            !super::read(&tree, &[]).unwrap().complete,
            "no commit on its branch yet"
        );
        fs::write(
            main.join("packed-refs"),
            "0123456789abcdef0123456789abcdef01234567 refs/heads/fix\n",
        )
        .unwrap();
        let read = super::read(&tree, &[]).unwrap();
        assert!(read.complete, "its branch is in the main repository's refs");
        assert!(read.worktree);
        assert_eq!(read.origin.as_deref(), Some("https://github.com/acme/web"));
    }

    #[test]
    fn a_separate_git_folder_is_no_worktree() {
        let home = tempfile::tempdir().unwrap();
        let git = home.path().join("store/web.git");
        fs::create_dir_all(&git).unwrap();
        fs::write(git.join("HEAD"), "0123456789abcdef0123456789abcdef01234567\n").unwrap();
        let repo = home.path().join("code/web");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join(".git"), format!("gitdir: {}\n", git.display())).unwrap();
        let read = super::read(&repo, &[]).unwrap();
        assert!(read.complete);
        assert!(!read.worktree, "cloned with --separate-git-dir, still its own checkout");
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

    #[test]
    fn a_guarded_folder_is_found_whatever_the_case() {
        assert!(super::below(
            std::path::Path::new("/Users/x/documents/code"),
            std::path::Path::new("/Users/x/Documents")
        ));
        assert!(!super::below(
            std::path::Path::new("/Users/x/code"),
            std::path::Path::new("/Users/x/Documents")
        ));
    }

    #[test]
    fn a_ref_that_names_no_object_is_no_commit() {
        let home = tempfile::tempdir().unwrap();
        let git = home.path().join(".git");
        fs::create_dir_all(git.join("refs/heads")).unwrap();
        fs::write(git.join("refs/heads/main"), "not-a-commit\n").unwrap();
        assert!(!super::head_resolves(&git, "ref: refs/heads/main", &[]));
    }

    #[test]
    fn a_reftable_repository_is_never_taken_as_finished() {
        let home = tempfile::tempdir().unwrap();
        let git = home.path().join(".git");
        fs::create_dir_all(git.join("reftable")).unwrap();
        fs::write(
            git.join("reftable/tables.list"),
            "0x000000000001-0x000000000002-1a2b3c4d.ref\n",
        )
        .unwrap();
        fs::write(git.join("HEAD"), "ref: refs/heads/.invalid\n").unwrap();
        let read = super::read(home.path(), &[]).unwrap();
        assert!(!read.complete, "its tables may hold no commit");
        assert_eq!(read.branch, None, "the placeholder is no branch");
    }

    #[test]
    fn no_search_starts_while_as_many_as_allowed_still_run() {
        use std::sync::atomic::Ordering;
        super::SEARCHING.fetch_add(super::SEARCHES, Ordering::AcqRel);
        let receiver = super::spawn_checkouts();
        super::SEARCHING.fetch_sub(super::SEARCHES, Ordering::AcqRel);
        assert!(receiver.is_none(), "no search starts; it is asked for again later");
    }
}

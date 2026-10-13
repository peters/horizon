//! The one field that finds the code: words search the checkouts on this computer and the
//! connected GitHub account's repositories at once, a path opens folders as it is typed,
//! and a link is cloned. The results answer the arrow keys, Enter and Tab.
mod fuzzy;
mod view;

use horizon_core::{
    cloud_runtime::repository::source,
    dir_search::{self, Checkout},
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::mpsc::Receiver,
    time::SystemTime,
};

/// The most results listed; the rest are a few more letters away.
const LISTED: usize = 60;

/// How many recent checkouts lead the list before anything is typed.
const RECENT: usize = 6;

/// One result.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Row {
    pub title: String,
    pub detail: String,
    /// Characters of `title` the query matched, for highlighting.
    pub marks: Vec<usize>,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Kind {
    /// A checkout on this computer; `github` when the connected account lists it too.
    Local { path: PathBuf, github: bool },
    /// A repository of the connected account with no checkout here yet.
    GitHub { name: String },
    /// A folder of a typed path; `checkout` when it holds a repository.
    Folder { path: PathBuf, checkout: bool },
}

/// What a chosen result asks of the field.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Pick {
    /// Put this text in the field and look it up.
    Text(String),
    /// Use this checkout now.
    Checkout(PathBuf),
}

#[derive(Default)]
pub(super) struct Finder {
    checkouts: Option<Vec<Checkout>>,
    /// Each checkout's title and detail, made once when the checkouts arrive.
    labels: Vec<(String, String)>,
    scan: Option<Receiver<Vec<Checkout>>>,
    /// The typed path folders were last listed for, and their answer: each folder and
    /// whether it holds a repository.
    lookup: Option<(String, Receiver<Folders>)>,
    folders_for: String,
    folders: Folders,
    /// What the rows were made from.
    made_for: Option<Made>,
    rows: Vec<Row>,
    pub highlighted: usize,
    /// The highlight moved by key, so the list scrolls to it.
    pub scroll_to_highlight: bool,
    /// The person moved the highlight with the arrow keys since the query changed.
    moved: bool,
    /// The rows are an earlier path's while this one's folders are read; keys wait.
    stale: bool,
}

/// The query, the checkout count, a digest of the repository names, and whether the
/// folders listed answer the query, with their count: what the rows were made from.
type Made = (String, usize, u64, bool, usize);

/// A digest of `names`, so a fresh list of as many names still makes the rows again.
fn digest(names: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    names.hash(&mut hasher);
    hasher.finish()
}

/// Folders of a typed path, each with whether it holds a repository.
type Folders = Vec<(PathBuf, bool)>;

/// Lists the folders of a typed path on a thread, with whether each holds a repository,
/// so no folder is read while a frame is drawn.
fn spawn_folders(query: String) -> Receiver<Folders> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let found = dir_search::spawn_lookup(query).recv().unwrap_or_default();
        let marked = found
            .into_iter()
            .map(|path| {
                let checkout = path.join(".git").exists();
                (path, checkout)
            })
            .collect();
        let _ = sender.send(marked);
    });
    receiver
}

/// Text that names a folder rather than a repository or a search.
pub(super) fn is_path(text: &str) -> bool {
    let text = text.trim_start();
    // `.github` is a name to search for; `./`, `../`, `.` and `..` are places.
    let relative = matches!(text, "." | "..") || text.starts_with("./") || text.starts_with("../");
    relative || text.starts_with(['/', '~', '\\']) || (cfg!(windows) && text.len() >= 2 && text.as_bytes()[1] == b':')
}

impl Finder {
    /// Starts looking for checkouts once, and takes them in when they arrive.
    pub fn poll(&mut self, ctx: &egui::Context, typed: &str) {
        // Tests never read the home folder of whoever runs them.
        if cfg!(test) && self.checkouts.is_none() {
            self.checkouts = Some(Vec::new());
        }
        if self.checkouts.is_none() && self.scan.is_none() {
            self.scan = Some(dir_search::spawn_checkouts());
        }
        if let Some(scan) = &self.scan {
            match scan.try_recv() {
                Ok(found) => {
                    self.labels = found
                        .iter()
                        .map(|checkout| (checkout_title(checkout), checkout_detail(checkout)))
                        .collect();
                    self.checkouts = Some(found);
                    self.scan = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.checkouts = Some(Vec::new());
                    self.scan = None;
                }
            }
        }
        let typed = typed.trim();
        if is_path(typed) {
            if self.lookup.as_ref().is_none_or(|(asked, _)| asked != typed) && self.folders_for != typed {
                // `.` and `..` name the home folder and its parent, as `./` and `../` do.
                let query = if matches!(typed, "." | "..") {
                    format!("{typed}/")
                } else {
                    typed.to_owned()
                };
                self.lookup = Some((typed.to_owned(), spawn_folders(query)));
            }
            if let Some((asked, receiver)) = &self.lookup {
                match receiver.try_recv() {
                    Ok(found) => {
                        self.folders_for.clone_from(asked);
                        self.folders = found;
                        self.lookup = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        ctx.request_repaint_after(std::time::Duration::from_millis(50));
                    }
                    // A lookup that ended without an answer found nothing.
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.folders_for.clone_from(asked);
                        self.folders.clear();
                        self.lookup = None;
                    }
                }
            }
        }
    }

    pub fn scanning(&self) -> bool {
        self.scan.is_some()
    }

    pub fn local_count(&self) -> usize {
        self.checkouts.as_ref().map_or(0, Vec::len)
    }

    /// The rows for `typed`, made again only when it or what is searched changes.
    pub fn rows(&mut self, typed: &str, repositories: &[String]) -> &[Row] {
        let key: Made = (
            typed.trim().to_owned(),
            self.local_count(),
            digest(repositories),
            self.folders_for == typed.trim(),
            self.folders.len(),
        );
        if self.made_for.as_ref() != Some(&key) {
            let previous = self.rows.get(self.highlighted).map(|row| row.kind.clone());
            // An earlier path's folders stay in view until this one's are read.
            self.stale = is_path(&key.0) && self.folders_for != key.0;
            self.rows = if is_path(&key.0) {
                if self.stale {
                    std::mem::take(&mut self.rows)
                } else {
                    folder_rows(&key.0, &self.folders)
                }
            } else {
                let checkouts = self.checkouts.as_deref().unwrap_or_default();
                // Tests set checkouts without labels; they are made here then.
                if self.labels.len() != checkouts.len() {
                    self.labels = checkouts
                        .iter()
                        .map(|checkout| (checkout_title(checkout), checkout_detail(checkout)))
                        .collect();
                }
                search_rows(&key.0, checkouts, &self.labels, repositories)
            };
            // A new query starts at the top; new rows for the same query keep the highlight on
            // the row it was on, wherever that row moved.
            if self.made_for.as_ref().is_none_or(|made| made.0 != key.0) {
                self.highlighted = 0;
                self.moved = false;
            } else if let Some(kind) = previous {
                self.highlighted = self.rows.iter().position(|row| row.kind.same_place(&kind)).unwrap_or(0);
            }
            self.highlighted = self.highlighted.min(self.rows.len().saturating_sub(1));
            self.made_for = Some(key);
        }
        &self.rows
    }

    pub fn current_rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn move_highlight(&mut self, down: bool) {
        self.scroll_to_highlight = true;
        self.moved = true;
        let last = self.rows.len().saturating_sub(1);
        self.highlighted = if down {
            (self.highlighted + 1).min(last)
        } else {
            self.highlighted.saturating_sub(1)
        };
    }

    /// Whether Tab opens the highlighted folder rather than moving on; while a path's
    /// folders are read it waits for them instead.
    pub fn takes_tab(&self) -> bool {
        self.stale
            || matches!(
                self.rows.get(self.highlighted).map(|row| &row.kind),
                Some(Kind::Folder { .. })
            )
    }

    /// Whether the rows answer what is typed now.
    pub fn settled(&self) -> bool {
        !self.stale
    }
}

/// Whether the text starts with a host name, as `gitlab.com/group/repo` does: a link to
/// clone, not a name to search for.
fn names_a_host(text: &str) -> bool {
    text.trim()
        .split('/')
        .next()
        .is_some_and(|first| first.contains('.') && !first.starts_with('.'))
}

impl Kind {
    /// Whether two results are the same place, whatever their badges say now.
    fn same_place(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Local { path: a, .. } | Self::Folder { path: a, .. },
                Self::Local { path: b, .. } | Self::Folder { path: b, .. },
            ) => a == b,
            (Self::GitHub { name: a }, Self::GitHub { name: b }) => a == b,
            _ => false,
        }
    }
}

/// What choosing `row` does; `open` (Tab) goes into a folder instead of using it.
pub(super) fn pick(row: &Row, open: bool) -> Pick {
    let shown = |path: &Path| dir_search::abbreviate_home(path);
    match &row.kind {
        Kind::Local { path, .. } => Pick::Checkout(path.clone()),
        Kind::GitHub { name } => Pick::Text(format!("https://github.com/{name}")),
        Kind::Folder { path, checkout: true } if !open => Pick::Checkout(path.clone()),
        Kind::Folder { path, .. } => Pick::Text(format!("{}{}", shown(path), std::path::MAIN_SEPARATOR)),
    }
}

/// `owner/name` of a GitHub address, in lowercase as the account lists it.
fn github_name(url: &str) -> Option<String> {
    let remote = source::parse(url)?;
    (remote.host == "github.com").then(|| format!("{}/{}", remote.owner, remote.name).to_ascii_lowercase())
}

/// The title a checkout is known by: where it came from, or its folder's name.
fn checkout_title(checkout: &Checkout) -> String {
    let folder = || {
        checkout.path.file_name().map_or_else(
            || checkout.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    checkout
        .origin
        .as_deref()
        .and_then(source::parse)
        .map_or_else(folder, |remote| {
            let path = format!("{}/{}", remote.owner, remote.name);
            if remote.host == "github.com" {
                path
            } else {
                format!("{}/{path}", remote.host)
            }
        })
}

fn checkout_detail(checkout: &Checkout) -> String {
    let mut parts = vec![dir_search::abbreviate_home(&checkout.path)];
    parts.extend(checkout.branch.clone());
    parts.extend(checkout.touched.and_then(ago));
    parts.join("  ·  ")
}

/// How long ago `when` was, in the largest unit that fits.
fn ago(when: SystemTime) -> Option<String> {
    let seconds = SystemTime::now().duration_since(when).ok()?.as_secs();
    Some(match seconds {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        86_400..172_800 => "yesterday".into(),
        172_800..1_209_600 => format!("{} days ago", seconds / 86_400),
        _ => format!("{} weeks ago", seconds / 604_800),
    })
}

/// Checkouts and repositories that match `query`, best first. Before anything is typed the
/// most recently changed checkouts lead, then every repository not cloned yet.
fn search_rows(query: &str, checkouts: &[Checkout], labels: &[(String, String)], repositories: &[String]) -> Vec<Row> {
    let listed: HashSet<&str> = repositories.iter().map(String::as_str).collect();
    let mut cloned = HashSet::new();
    let mut scored: Vec<(i32, Row)> = Vec::new();
    for (recency, (checkout, (title, detail))) in checkouts.iter().zip(labels).enumerate() {
        let name = checkout.origin.as_deref().and_then(github_name);
        let github = name.as_deref().is_some_and(|name| listed.contains(name));
        if github && let Some(name) = name {
            cloned.insert(name);
        }
        let (score, marks) = if query.is_empty() {
            if recency >= RECENT {
                continue;
            }
            (-i32::try_from(recency).unwrap_or(i32::MAX), Vec::new())
        } else if let Some(found) = fuzzy::score(query, title) {
            // Being on this computer already is worth a little.
            (found.score + 8, found.positions)
        } else if dir_search::abbreviate_home(&checkout.path)
            .to_lowercase()
            .contains(&query.to_lowercase())
        {
            // Typed letters that only appear scattered over a path are noise; a run of them
            // in the path, such as `projects/tool`, is not.
            (-40, Vec::new())
        } else {
            continue;
        };
        let kind = Kind::Local {
            path: checkout.path.clone(),
            github,
        };
        scored.push((
            score,
            Row {
                title: title.clone(),
                detail: detail.clone(),
                marks,
                kind,
            },
        ));
    }
    for name in repositories.iter().filter(|name| !cloned.contains(name.as_str())) {
        let (score, marks) = if query.is_empty() {
            (-1000, Vec::new())
        } else if let Some(found) = fuzzy::score(query, name) {
            (found.score, found.positions)
        } else {
            continue;
        };
        let row = Row {
            title: name.clone(),
            detail: "github.com  ·  not on this computer yet".into(),
            marks,
            kind: Kind::GitHub { name: name.clone() },
        };
        scored.push((score, row));
    }
    // A stable sort keeps recent checkouts and the account's order among equals.
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().take(LISTED).map(|(_, row)| row).collect()
}

/// The folders of a typed path: repositories first, then by name. Hidden folders show
/// only once a dot is typed.
fn folder_rows(typed: &str, folders: &[(PathBuf, bool)]) -> Vec<Row> {
    let dotted = typed
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|last| last.starts_with('.'));
    let name = |path: &Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    let mut shown: Vec<(bool, &PathBuf)> = folders
        .iter()
        .filter(|(path, _)| dotted || !name(path).starts_with('.'))
        .map(|(path, checkout)| (*checkout, path))
        .collect();
    // The folder named as typed first, then those that start so, then repositories.
    let typed_name = typed.rsplit(['/', '\\']).next().unwrap_or_default().to_lowercase();
    let rank = |checkout: bool, path: &Path| {
        let name = name(path);
        (
            name == typed_name,
            !typed_name.is_empty() && name.starts_with(&typed_name),
            checkout,
        )
    };
    shown.sort_by(|a, b| {
        rank(b.0, b.1)
            .cmp(&rank(a.0, a.1))
            .then_with(|| name(a.1).cmp(&name(b.1)))
    });
    shown
        .into_iter()
        .take(LISTED)
        .map(|(checkout, path)| Row {
            title: path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            detail: dir_search::abbreviate_home(path),
            marks: Vec::new(),
            kind: Kind::Folder {
                path: path.clone(),
                checkout,
            },
        })
        .collect()
}

impl super::State {
    /// The finder below the field: shown while nothing is being cloned and the field holds
    /// no pasted link or chosen folder. Returns a checkout to use right away.
    pub(super) fn finder(&mut self, ui: &mut egui::Ui) -> Option<PathBuf> {
        if !self.searching() {
            return None;
        }
        let picked = view::show(ui, self)?;
        self.apply(picked)
    }

    /// Makes the rows for what the field holds now, before keys and the focus lock use them.
    pub(super) fn update_rows(&mut self) {
        let typed = self.input.trim().to_owned();
        let repositories = self
            .account
            .as_ref()
            .and_then(|account| account.repositories.as_deref())
            .unwrap_or_default();
        let _ = self.finder.rows(&typed, repositories);
    }

    /// Arrow keys, Enter and Tab for the results, taken before the field sees them.
    pub(super) fn finder_keys(&mut self, ui: &egui::Ui, focused: bool) -> Option<PathBuf> {
        use egui::{Key, Modifiers};
        // Shift+Tab and other chords keep their own meaning, and an earlier path's rows take
        // no keys while this path's folders are read.
        let plain = ui.input(|input| input.modifiers.is_none());
        if !focused || !plain || !self.searching() || !self.finder.settled() || self.finder.current_rows().is_empty() {
            return None;
        }
        // Enter on a typed `owner/name` clones what was typed, unless the person picked a row
        // with the arrow keys or the highlighted row is exactly what was typed.
        let exact = self
            .finder
            .current_rows()
            .get(self.finder.highlighted)
            .is_some_and(|row| row.title.eq_ignore_ascii_case(self.input.trim()));
        let takes_enter = self.remote.is_none() || self.finder.moved || exact;
        let (down, up, enter, tab) = ui.input_mut(|input| {
            (
                input.consume_key(Modifiers::NONE, Key::ArrowDown),
                input.consume_key(Modifiers::NONE, Key::ArrowUp),
                takes_enter && input.consume_key(Modifiers::NONE, Key::Enter),
                self.finder.takes_tab() && input.consume_key(Modifiers::NONE, Key::Tab),
            )
        });
        if down || up {
            self.finder.move_highlight(down);
        }
        if !(enter || tab) {
            return None;
        }
        let row = self.finder.current_rows().get(self.finder.highlighted)?.clone();
        self.apply(pick(&row, tab))
    }

    /// Whether the field is a search: nothing is being cloned, and it holds neither a
    /// pasted link, a folder with a repository, nor the repository already chosen.
    pub(super) fn searching(&self) -> bool {
        let shows_chosen = !self.input.trim().is_empty() && !self.editing();
        let link = self.remote.is_some() && (super::github::names_a_link(&self.input) || names_a_host(&self.input));
        self.job.is_none() && self.folder.is_none() && !shows_chosen && !link
    }

    fn apply(&mut self, picked: Pick) -> Option<PathBuf> {
        match picked {
            Pick::Text(text) => {
                self.input = text;
                self.refocus = true;
                None
            }
            Pick::Checkout(path) => {
                self.input = dir_search::abbreviate_home(&path);
                Some(path)
            }
        }
    }
}

#[cfg(test)]
mod tests;

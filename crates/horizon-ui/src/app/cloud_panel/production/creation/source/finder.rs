//! The one field that finds the code: words search the checkouts on this computer and the
//! connected GitHub account's repositories at once, a path opens folders as it is typed,
//! and a link is cloned. The results answer the arrow keys, Enter and Tab.
mod folders;
mod fuzzy;
mod rows;
mod view;

use folders::Folders;
use horizon_core::{
    cloud_runtime::repository::source,
    dir_search::{self, Checkout},
};
use rows::{folder_rows, search_rows};
use std::{
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
    /// The GitHub repository the row stands for, in lowercase `owner/name`, whether it is
    /// listed or cloned: the same repository across a list that changes.
    pub repo: Option<String>,
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
    /// The search for checkouts started; it runs once per dialog.
    scanned: bool,
    scan: Option<Receiver<Vec<Checkout>>>,
    /// How many answers the search for checkouts has given.
    snapshots: usize,
    /// The search waits to start: earlier dialogs' searches still run.
    waiting: bool,
    /// The listing of a typed path's folders, and the path the folders below answer.
    lookup: folders::Lookup,
    folders_for: String,
    folders: Folders,
    /// How many answers the folders have had: one path may be answered twice, first with
    /// no folders while every listing thread is busy.
    answers: usize,
    /// The repository list the digest below was made for, by place and length, and the
    /// digest: a list is hashed again only when it is another one.
    digested: Option<((usize, usize), u64)>,
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

/// The query, what the search had found, a digest of the repository names, and whether the
/// folders listed answer the query, with how many answers they had: what the rows were
/// made from.
type Made = (String, (usize, usize), u64, bool, usize);

/// A digest of `names`, so a fresh list of as many names still makes the rows again.
fn digest(names: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    names.hash(&mut hasher);
    hasher.finish()
}

/// Whether `query` narrows a list that holds `name`: it is part of the name, whatever the
/// case. Not the looser matching of the results: almost any short `owner/name` has its
/// letters in order in some listed name of a large account, and text that narrows the list
/// is not looked up as a repository, so an unlisted one could then only be pasted as a link.
pub(super) fn narrows(query: &str, name: &str) -> bool {
    name.to_lowercase().contains(&query.trim().to_lowercase())
}

/// Text that names a folder rather than a repository or a search.
pub(super) fn is_path(text: &str) -> bool {
    let text = text.trim_start();
    // `.github` is a name to search for; `./`, `../`, `.` and `..` are places, and on
    // Windows `.\` and `..\` too.
    let relative = matches!(text, "." | "..")
        || ["./", "../"].iter().any(|start| text.starts_with(start))
        || (cfg!(windows) && [".\\", "..\\"].iter().any(|start| text.starts_with(start)));
    // `~` and `~/...` are the home folder; `~alice` is a word to search for, as in the lookup.
    let home = text
        .strip_prefix('~')
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/') || (cfg!(windows) && rest.starts_with('\\')));
    let windows = cfg!(windows) && (text.starts_with('\\') || (text.len() >= 2 && text.as_bytes()[1] == b':'));
    relative || home || text.starts_with('/') || windows
}

impl Finder {
    /// Starts looking for checkouts once, and takes them in when they arrive.
    pub fn poll(&mut self, ctx: &egui::Context, typed: &str) {
        // Tests never read the home folder of whoever runs them.
        if cfg!(test) && self.checkouts.is_none() {
            self.checkouts = Some(Vec::new());
        }
        if self.checkouts.is_none() && self.scan.is_none() && !self.scanned {
            self.scan = dir_search::spawn_checkouts();
            self.scanned = self.scan.is_some();
            // Earlier dialogs' searches still hold every place: asked again in a while.
            self.waiting = !self.scanned;
            if self.waiting {
                ctx.request_repaint_after(std::time::Duration::from_secs(1));
            }
        }
        // What the search found so far shows at once; the search is done when it closes.
        if let Some(scan) = &self.scan {
            let mut latest = None;
            let mut done = false;
            loop {
                match scan.try_recv() {
                    Ok(found) => latest = Some(found),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        done = true;
                        break;
                    }
                }
            }
            // The search tells what it has every second; the rows are made again only when that
            // changed, so a list scrolled by hand stays where it is.
            if let Some(found) = latest.filter(|found| self.checkouts.as_ref() != Some(found)) {
                self.labels = found
                    .iter()
                    .map(|checkout| (checkout_title(checkout), checkout_detail(checkout)))
                    .collect();
                self.checkouts = Some(found);
                self.snapshots += 1;
            }
            if done {
                self.checkouts.get_or_insert_with(Vec::new);
                self.scan = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        let typed = typed.trim();
        if let Some((asked, found)) = self.lookup.poll(ctx, typed, &self.folders_for) {
            self.folders_for = asked;
            self.folders = found;
            self.answers += 1;
        }
    }

    /// A digest of `names`, kept while the same list is shown.
    fn digest(&mut self, names: &[String]) -> u64 {
        let key = (names.as_ptr() as usize, names.len());
        match self.digested {
            Some((made, value)) if made == key => value,
            _ => {
                let value = digest(names);
                self.digested = Some((key, value));
                value
            }
        }
    }

    /// Whether the folders of a typed path are being read.
    pub fn looking(&self) -> bool {
        self.lookup.looking(&self.folders_for)
    }

    /// The path of [`Self::found_checkout`].
    #[cfg(test)]
    pub fn checkout_of(&self, remote: &source::Remote) -> Option<PathBuf> {
        self.found_checkout(remote).map(|checkout| checkout.path.clone())
    }

    /// Puts `checkouts` in as if the search had found them, still searching when `scanning`.
    #[cfg(test)]
    pub fn found_for_test(&mut self, checkouts: Vec<Checkout>, scanning: bool) {
        self.labels = checkouts
            .iter()
            .map(|checkout| (checkout_title(checkout), checkout_detail(checkout)))
            .collect();
        self.checkouts = Some(checkouts);
        self.snapshots += 1;
        self.scanned = true;
        self.scan = scanning.then(|| std::sync::mpsc::channel().1);
    }

    /// A checkout found on this computer of the repository `remote` names: a finished one,
    /// not one with no commit or a clone that stopped. The repository's own checkout comes
    /// before a worktree, whose branch and changes are another piece of work.
    pub fn found_checkout(&self, remote: &source::Remote) -> Option<&Checkout> {
        let same = |origin: &str| source::parse(origin).is_some_and(|found| source::same_repository(&found, remote));
        let found = self.checkouts.as_deref()?;
        let usable = || {
            found
                .iter()
                .filter(|checkout| checkout.complete && checkout.origin.as_deref().is_some_and(same))
        };
        usable().find(|checkout| !checkout.worktree).or_else(|| usable().next())
    }

    /// Whether checkouts may still turn up: the search runs, or waits for earlier dialogs'
    /// searches to give way.
    pub fn scanning(&self) -> bool {
        self.scan.is_some() || self.waiting
    }

    /// What the search has found so far, as a number that changes with every new answer:
    /// rows and plans made from the checkouts are made again when it changes.
    pub fn found_version(&self) -> (usize, usize) {
        (self.snapshots, self.checkouts.as_ref().map_or(0, Vec::len))
    }

    /// How many checkouts were found that can be results: a clone that stopped is none.
    pub fn local_count(&self) -> usize {
        self.checkouts
            .as_deref()
            .map_or(0, |found| found.iter().filter(|checkout| !checkout.stopped).count())
    }

    /// The rows for `typed`, made again only when it or what is searched changes.
    pub fn rows(&mut self, typed: &str, repositories: &[String]) -> &[Row] {
        let key: Made = (
            typed.trim().to_owned(),
            self.found_version(),
            self.digest(repositories),
            self.folders_for == typed.trim(),
            self.answers,
        );
        if self.made_for.as_ref() != Some(&key) {
            let previous = self
                .rows
                .get(self.highlighted)
                .map(|row| (row.kind.clone(), row.repo.clone()));
            // An earlier path's folders stay in view until this one's are read.
            self.stale = is_path(&key.0) && self.folders_for != key.0;
            self.rows = if is_path(&key.0) {
                if self.stale {
                    std::mem::take(&mut self.rows)
                } else {
                    let mut rows = Vec::new();
                    // A typed folder that is itself a repository can be used from here.
                    if let Some(here) = &self.folders.0 {
                        rows.push(Row {
                            title: "Use this repository".into(),
                            detail: dir_search::abbreviate_home(here),
                            marks: Vec::new(),
                            kind: Kind::Local {
                                path: here.clone(),
                                github: false,
                            },
                            repo: None,
                        });
                    }
                    rows.extend(folder_rows(&key.0, &self.folders.1));
                    rows
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
            } else if let Some((kind, repo)) = previous.filter(|_| self.moved) {
                // The same place, or the same repository now that it is cloned or listed.
                let same = |row: &Row| row.kind.same_place(&kind) || (repo.is_some() && row.repo == repo);
                self.highlighted = self.rows.iter().position(same).unwrap_or(0);
            } else {
                // Not chosen by the person: the best result leads.
                self.highlighted = 0;
            }
            self.highlighted = self.highlighted.min(self.rows.len().saturating_sub(1));
            // The list shows its highlight, wherever it scrolled before.
            self.scroll_to_highlight = true;
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

/// `owner/name` of a GitHub address, in lowercase as the account lists it: only for the
/// repository the account's row stands for, reached over a transport's usual port and not
/// over another port of the same host.
fn github_name(url: &str) -> Option<String> {
    let remote = source::parse(url)?;
    github_path(&remote).map(|name| name.to_ascii_lowercase())
}

/// `owner/name` of `remote` when it is a repository on GitHub itself, as written.
fn github_path(remote: &source::Remote) -> Option<String> {
    let name = format!("{}/{}", remote.owner, remote.name);
    // Ports are compared over one transport only, so each transport's usual form is asked.
    let usual = [
        format!("https://github.com/{name}"),
        format!("git@github.com:{name}"),
        format!("git://github.com/{name}"),
    ];
    usual
        .iter()
        .all(|address| source::parse(address).is_some_and(|listed| source::same_repository(remote, &listed)))
        .then_some(name)
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
            github_path(&remote).unwrap_or_else(|| {
                // A host on another port is another server, so the port is part of the title.
                let place = remote
                    .url
                    .split_once("://")
                    .and_then(|(_, rest)| rest.split('/').next())
                    .map_or(remote.host.as_str(), |authority| {
                        authority.rsplit('@').next().unwrap_or(authority)
                    });
                format!("{place}/{}/{}", remote.owner, remote.name)
            })
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

    /// Takes in what the background searches found and makes the rows for what the field
    /// holds now, before keys and the focus lock use them.
    pub(super) fn update_rows(&mut self, ctx: &egui::Context) {
        let typed = self.input.trim().to_owned();
        // The search for checkouts goes on also once a repository is chosen, so a clone can
        // still find that it is already on this computer.
        self.finder.poll(ctx, if self.searching() { &typed } else { "" });
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
        // no keys while this path's folders are read. Text typed, pasted or committed by an
        // input method in this frame
        // reaches the field only after these keys, so the rows would answer the text before
        // it: the field and Continue take such a frame's keys instead.
        let plain = ui.input(|input| {
            input.modifiers.is_none()
                && !input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Ime(egui::ImeEvent::Commit(_))
                    )
                })
        });
        // The account's list may have changed earlier in this frame, and with it whether the
        // text is an `owner/name` to clone.
        let typed_repository = self.remote().is_some();
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
        let takes_enter = !typed_repository || self.finder.moved || exact;
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

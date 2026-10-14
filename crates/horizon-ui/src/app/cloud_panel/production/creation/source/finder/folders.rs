//! The folders of a typed path, listed on a thread: one listing at a time, started once
//! the path has not changed for a moment, so typing a long path starts no pile of scans.
use super::is_path;
use horizon_core::dir_search;
use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, TryRecvError},
    time::{Duration, Instant},
};

/// How long a typed path stays the same before its folders are listed.
const SETTLE: Duration = Duration::from_millis(150);

/// The folders of a typed path, each with whether it holds a repository, and the typed
/// folder itself when it ends in a separator and holds one.
pub(super) type Folders = (Option<PathBuf>, Vec<(PathBuf, bool)>);

/// The listing that runs, if any, and the path last typed with when it changed.
#[derive(Default)]
pub(super) struct Lookup {
    running: Option<(String, Receiver<Folders>)>,
    typed: Option<(String, Instant)>,
}

impl Lookup {
    /// Takes in a finished listing, and starts one for `typed` once it has stayed the same
    /// for [`SETTLE`] and its folders are not `answered` yet. A listing for a path no longer
    /// typed is let go then, so a slow folder holds up no other. Returns the path a listing
    /// finished for, with its folders.
    pub fn poll(&mut self, ctx: &egui::Context, typed: &str, answered: &str) -> Option<(String, Folders)> {
        // A listing for a path no longer typed is dropped, and with it its spinner.
        if !is_path(typed) {
            self.running = None;
            self.typed = None;
            return None;
        }
        if self.typed.as_ref().is_none_or(|(last, _)| last != typed) {
            self.typed = Some((typed.to_owned(), Instant::now()));
        }
        let mut finished = None;
        if let Some((asked, receiver)) = &self.running {
            match receiver.try_recv() {
                Ok(found) => finished = Some((asked.clone(), found)),
                Err(TryRecvError::Empty) if asked == typed || !self.settled() => {
                    ctx.request_repaint_after(Duration::from_millis(50));
                    return None;
                }
                Err(TryRecvError::Empty) => {}
                // A listing that ended without an answer found nothing.
                Err(TryRecvError::Disconnected) => finished = Some((asked.clone(), (None, Vec::new()))),
            }
            self.running = None;
        }
        let answered = finished.as_ref().map_or(answered, |(asked, _)| asked.as_str());
        if answered != typed {
            if let Some(left) = self.left() {
                ctx.request_repaint_after(left);
            } else {
                self.running = Some((typed.to_owned(), spawn_folders(folder_query(typed))));
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        finished
    }

    /// How long the typed path is still to stay the same before it is listed, if at all.
    fn left(&self) -> Option<Duration> {
        let waited = self.typed.as_ref().map_or(SETTLE, |(_, at)| at.elapsed());
        SETTLE.checked_sub(waited).filter(|left| !left.is_zero())
    }

    fn settled(&self) -> bool {
        self.left().is_none()
    }

    /// Whether the typed path's folders are still to come.
    pub fn looking(&self, answered: &str) -> bool {
        self.running.is_some() || self.typed.as_ref().is_some_and(|(typed, _)| typed != answered)
    }
}

/// Lists the folders of a typed path on a thread, with whether each holds a repository,
/// so no folder is read while a frame is drawn.
fn spawn_folders(query: String) -> Receiver<Folders> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let here = query
            .ends_with(std::path::is_separator)
            .then(|| dir_search::resolve_query_path(&query))
            .flatten()
            .filter(|path| path.join(".git").exists());
        let found = dir_search::spawn_lookup(query).recv().unwrap_or_default();
        let marked = found
            .into_iter()
            .map(|path| {
                let checkout = path.join(".git").exists();
                (path, checkout)
            })
            .collect();
        let _ = sender.send((here, marked));
    });
    receiver
}

/// What to list for a typed path: `~`, `.` and `..` name folders whose own folders are
/// listed, as `~/`, `./` and `../` do, and a final `/.` lists the folder before it, its
/// hidden folders included.
pub(super) fn folder_query(typed: &str) -> String {
    if matches!(typed, "~" | "." | "..") {
        format!("{typed}/")
    } else if let Some(folder) = typed
        .strip_suffix('.')
        .filter(|rest| rest.ends_with(std::path::is_separator))
    {
        folder.to_owned()
    } else {
        typed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{Lookup, SETTLE};
    use std::time::{Duration, Instant};

    #[test]
    fn a_listing_that_hangs_holds_up_no_other_path() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("code")).unwrap();
        let ctx = egui::Context::default();
        // A listing that never answers, as one of a folder on a mount that hangs.
        let (_hung, receiver) = std::sync::mpsc::channel();
        let typed = format!("{}/", home.path().display());
        let mut lookup = Lookup {
            running: Some(("/hung/".into(), receiver)),
            typed: Some((typed.clone(), Instant::now().checked_sub(SETTLE).unwrap())),
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some((asked, found)) = lookup.poll(&ctx, &typed, "") {
                assert_eq!(asked, typed);
                assert!(found.1.iter().any(|(path, _)| path.ends_with("code")), "{found:?}");
                break;
            }
            assert!(Instant::now() < deadline, "the typed path was never listed");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!lookup.looking(&typed));
    }
}

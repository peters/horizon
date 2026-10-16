//! The folders of a typed path, listed on a thread: one listing at a time, started once
//! the path has not changed for a moment, so typing a long path starts no pile of scans.
use super::is_path;
use horizon_core::dir_search;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};

/// How long a typed path stays the same before its folders are listed.
const SETTLE: Duration = Duration::from_millis(150);

/// The most listings whose threads still run, those let go included: a folder on a mount
/// that hangs keeps its thread, and typing on starts no more than this.
const WORKERS: usize = 4;

/// The folders of a typed path, each with whether it holds a repository, and the typed
/// folder itself when it ends in a separator and holds one.
pub(super) type Folders = (Option<PathBuf>, Vec<(PathBuf, bool)>);

/// The listing that runs, if any, and the path last typed with when it changed.
#[derive(Default)]
pub(super) struct Lookup {
    running: Option<(String, Receiver<Folders>)>,
    typed: Option<(String, Instant)>,
    /// How many listing threads still run, the ones let go included.
    workers: Arc<AtomicUsize>,
    /// A path given no folders because every listing thread was busy, listed again once one
    /// is free.
    deferred: Option<String>,
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
            // A path put off stays so: typed again, it is listed then.
            return None;
        }
        if self.typed.as_ref().is_none_or(|(last, _)| last != typed) {
            self.typed = Some((typed.to_owned(), Instant::now()));
        }
        if let Some((asked, receiver)) = &self.running {
            let finished = match receiver.try_recv() {
                Ok(found) => (asked.clone(), found),
                // A listing that ended without an answer found nothing.
                Err(TryRecvError::Disconnected) => (asked.clone(), (None, Vec::new())),
                Err(TryRecvError::Empty) if asked == typed || !self.settled() => {
                    ctx.request_repaint_after(Duration::from_millis(50));
                    return None;
                }
                // Let go: its thread runs on and is counted until it ends. A path typed back
                // to the one already answered keeps its folders, unless they were only put
                // off for a free thread.
                Err(TryRecvError::Empty) => {
                    self.running = None;
                    let kept = answered == typed && self.deferred.as_deref() != Some(typed);
                    return if kept { None } else { self.start(ctx, typed) };
                }
            };
            self.running = None;
            // A listing that finished is taken in as it is; the next frame lists what is typed.
            ctx.request_repaint();
            return Some(finished);
        }
        if answered != typed || self.deferred.as_deref() == Some(typed) {
            if let Some(left) = self.left() {
                ctx.request_repaint_after(left);
                return None;
            }
            return self.start(ctx, typed);
        }
        None
    }

    /// Starts listing `typed`. With every listing thread busy the path is given no folders
    /// for now, so the field and its keys work on, and it is listed once a thread is free.
    fn start(&mut self, ctx: &egui::Context, typed: &str) -> Option<(String, Folders)> {
        if self.workers.load(Ordering::Acquire) < WORKERS {
            // A path put off stays so until it is listed itself.
            if self.deferred.as_deref() == Some(typed) {
                self.deferred = None;
            }
            let workers = Arc::clone(&self.workers);
            self.running = Some((typed.to_owned(), spawn_folders(folder_query(typed), workers)));
            ctx.request_repaint_after(Duration::from_millis(50));
            return None;
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        if self.deferred.as_deref() == Some(typed) {
            return None;
        }
        self.deferred = Some(typed.to_owned());
        Some((typed.to_owned(), (None, Vec::new())))
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
fn spawn_folders(query: String, workers: Arc<AtomicUsize>) -> Receiver<Folders> {
    /// Counts the thread out however it ends.
    struct Running(Arc<AtomicUsize>);
    impl Drop for Running {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::AcqRel);
        }
    }
    workers.fetch_add(1, Ordering::AcqRel);
    let running = Running(workers);
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let here = query
            .ends_with(std::path::is_separator)
            .then(|| dir_search::resolve_query_path(&query))
            .flatten()
            .filter(|path| usable(path));
        // Read on this thread, the one counted, rather than on one more.
        let found = dir_search::lookup(&query);
        let marked = found
            .into_iter()
            .map(|path| {
                let checkout = usable(&path);
                (path, checkout)
            })
            .collect();
        // Counted out before the answer goes, so whoever takes it in may start another.
        drop(running);
        let _ = sender.send((here, marked));
    });
    receiver
}

/// Whether `path` holds a repository to use: read as the search for checkouts reads one,
/// and not a clone that stopped halfway, which is a plain folder here.
fn usable(path: &std::path::Path) -> bool {
    dir_search::checkout_in(path).is_some_and(|checkout| !checkout.stopped)
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
            ..Lookup::default()
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

    #[test]
    fn listings_that_hang_start_no_more_threads_than_the_limit() {
        let ctx = egui::Context::default();
        let settled = || Instant::now().checked_sub(SETTLE).unwrap();
        let (_hung, receiver) = std::sync::mpsc::channel();
        // As many listing threads as allowed still run, each on a folder that hangs; the
        // last one was for a path no longer typed.
        let mut lookup = Lookup {
            running: Some(("/hung/".into(), receiver)),
            typed: Some(("/elsewhere/".into(), settled())),
            ..Lookup::default()
        };
        lookup
            .workers
            .store(super::WORKERS, std::sync::atomic::Ordering::Release);
        let typed = "/elsewhere/";
        let answered = lookup.poll(&ctx, typed, "/hung/");
        assert!(
            lookup.running.is_none(),
            "the stale listing is let go, and no thread starts"
        );
        assert_eq!(
            answered,
            Some((typed.to_owned(), (None, Vec::new()))),
            "the path is given no folders for now, so its keys work on"
        );
        assert!(!lookup.looking(typed));
        assert_eq!(lookup.poll(&ctx, typed, typed), None, "it waits for a free thread");
        lookup.workers.store(0, std::sync::atomic::Ordering::Release);
        lookup.poll(&ctx, typed, typed);
        assert!(lookup.running.is_some(), "the path is listed once a thread is free");
    }

    #[test]
    fn a_path_put_off_is_listed_when_typed_back() {
        let ctx = egui::Context::default();
        let (_hung, receiver) = std::sync::mpsc::channel();
        // `/a/` was given no folders while every thread was busy; `/ab` is being listed.
        let mut lookup = Lookup {
            running: Some(("/ab".into(), receiver)),
            typed: Some(("/a/".into(), Instant::now().checked_sub(SETTLE).unwrap())),
            deferred: Some("/a/".into()),
            ..Lookup::default()
        };
        assert_eq!(lookup.poll(&ctx, "/a/", "/a/"), None);
        assert!(
            lookup.running.as_ref().is_some_and(|(asked, _)| asked == "/a/"),
            "the path is listed now that a thread is free"
        );
    }

    #[test]
    fn a_path_put_off_stays_so_across_words_typed_between() {
        let ctx = egui::Context::default();
        let mut lookup = Lookup {
            deferred: Some("/a/".into()),
            ..Lookup::default()
        };
        assert_eq!(lookup.poll(&ctx, "acme", "/a/"), None);
        lookup.typed = Some(("/a/".into(), Instant::now().checked_sub(SETTLE).unwrap()));
        lookup.poll(&ctx, "/a/", "/a/");
        assert!(lookup.running.is_some(), "typed again, the path is listed");
    }

    #[test]
    fn a_path_typed_back_keeps_the_folders_it_had() {
        let ctx = egui::Context::default();
        let (_hung, receiver) = std::sync::mpsc::channel();
        let mut lookup = Lookup {
            running: Some(("/a/".into(), receiver)),
            typed: Some(("/b/".into(), Instant::now().checked_sub(SETTLE).unwrap())),
            ..Lookup::default()
        };
        lookup
            .workers
            .store(super::WORKERS, std::sync::atomic::Ordering::Release);
        assert_eq!(lookup.poll(&ctx, "/b/", "/b/"), None, "its folders stay as they were");
        assert!(lookup.running.is_none() && !lookup.looking("/b/"));
    }

    #[test]
    fn a_clone_that_stopped_is_a_plain_folder() {
        let home = tempfile::tempdir().unwrap();
        let stopped = home.path().join("web");
        std::fs::create_dir_all(stopped.join(".git")).unwrap();
        std::fs::write(
            stopped
                .join(".git")
                .join(horizon_core::cloud_runtime::repository::source::CLONE_MARKER),
            "https://github.com/acme/web.git\nmain\n1\n",
        )
        .unwrap();
        assert!(!super::usable(&stopped), "a partial checkout is not offered");
        let fresh = home.path().join("api");
        std::fs::create_dir_all(fresh.join(".git")).unwrap();
        assert!(super::usable(&fresh));
    }
}

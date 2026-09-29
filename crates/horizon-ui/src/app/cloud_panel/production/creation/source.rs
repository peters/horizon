//! Where a new cloud's code comes from: a pasted link is cloned, a folder is used as it is.
//! Git's own credentials do the signing in; a token is asked for only when Git has none.
mod view;

use egui::Context;
use horizon_core::cloud_runtime::{
    Cancellation,
    repository::source::{self, Failure, Progress, Remote, Token},
};
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, TryRecvError, channel},
    time::{Duration, Instant},
};
use zeroize::Zeroize;

pub(super) use view::{Step, continue_footer, step};

/// Where a link would be cloned and whether that checkout is already there.
struct Plan {
    url: String,
    parent: PathBuf,
    existing: Option<PathBuf>,
    destination: PathBuf,
}

/// How long a link must stand still before its access is looked up.
const SETTLE: Duration = Duration::from_millis(500);

/// A finished clone: where it is, and what to tell about keeping the token, if that was asked.
struct Cloned {
    path: PathBuf,
    note: Option<&'static str>,
}

struct Job {
    receiver: Receiver<Result<Cloned, Failure>>,
    cancel: Cancellation,
    progress: Progress,
}

#[derive(Default)]
pub(in crate::app::cloud_panel::production) struct State {
    input: String,
    /// The repository the text field last mirrored, and the text it was given.
    repository: String,
    mirrored: String,
    parsed_for: String,
    remote: Option<Remote>,
    folder: Option<PathBuf>,
    parent: Option<PathBuf>,
    /// The folder picker is choosing where clones go rather than a repository.
    choosing_parent: bool,
    job: Option<Job>,
    ready: Option<PathBuf>,
    failure: Option<Failure>,
    /// Something about the last clone worth saying once it is done.
    note: Option<&'static str>,
    token: String,
    token_tried: bool,
    token_focused: bool,
    /// Keep the token in Git's own credential helper after this clone.
    remember: bool,
    /// Where the current link would be cloned, worked out once per link and folder.
    plan: Option<Plan>,
    /// The link was read without signing in, so no token is asked for.
    public: bool,
    probe: Option<Receiver<Result<(), Failure>>>,
    /// Ends the probe on its way when the link it asked about is gone.
    probe_cancel: Cancellation,
    probed: Option<String>,
    changed: Option<Instant>,
}

impl State {
    /// Shows the chosen repository in the field, whoever chose it.
    fn mirror(&mut self, repository: &str) {
        if self.repository != repository {
            repository.clone_into(&mut self.repository);
            self.mirrored = if repository.is_empty() {
                String::new()
            } else {
                horizon_core::dir_search::abbreviate_home(Path::new(repository))
            };
            self.input.clone_from(&self.mirrored);
        }
    }

    #[cfg(all(test, unix))]
    pub fn input(&self) -> &str {
        &self.input
    }

    #[cfg(all(test, unix))]
    pub fn edit_for_test(&mut self, text: &str) {
        self.input = text.into();
    }

    /// The text differs from the repository chosen, so another one is being asked for. A field
    /// that was cleared is no exception: the old repository is not what it shows.
    pub fn editing(&self) -> bool {
        self.input.trim() != self.mirrored.trim()
    }

    /// The text names the repository already chosen, only written another way (`~/x` for its
    /// full path): it is what the field shows now, not a request for another repository.
    fn accept_text(&mut self) {
        self.mirrored.clone_from(&self.input);
    }

    /// The repository a pasted link names, unless the text is a folder that exists.
    fn remote(&mut self) -> Option<&Remote> {
        if self.parsed_for != self.input {
            self.parsed_for.clone_from(&self.input);
            let path = horizon_core::dir_search::expand_tilde(self.input.trim());
            let before = self.remote.as_ref().map(|remote| origin(&remote.url).to_owned());
            self.remote = source::parse(&self.input).filter(|_| !path.exists());
            // A token was pasted for one origin (scheme, host and port) and goes to no other.
            if before.as_deref() != self.remote.as_ref().map(|remote| origin(&remote.url)) {
                self.token.zeroize();
                self.token_tried = false;
                self.token_focused = false;
            }
            self.folder = (!self.input.trim().is_empty() && path.join(".git").exists()).then_some(path);
            self.failure = None;
            self.public = false;
            self.probe_cancel.cancel();
            self.probe = None;
            self.probed = None;
            self.changed = Some(Instant::now());
        }
        self.remote.as_ref()
    }

    /// The typed text, when it is a folder that holds a repository and the text has stood still:
    /// a path on its way to a longer one is not taken at the first repository it passes.
    fn folder_settled(&mut self, ctx: &Context) -> Option<PathBuf> {
        self.remote();
        let folder = self.folder.as_ref()?;
        let quiet = self.changed.map_or(SETTLE, |at| at.elapsed());
        if quiet < SETTLE {
            ctx.request_repaint_after(SETTLE.saturating_sub(quiet));
            return None;
        }
        Some(folder.clone())
    }

    /// Text that is neither a link nor a folder, once it is long enough to be judged.
    fn unrecognised(&mut self) -> bool {
        let text = self.input.trim();
        text.len() > 3
            && self.remote().is_none()
            && self.folder.is_none()
            && !horizon_core::dir_search::expand_tilde(self.input.trim()).is_dir()
    }

    fn parent(&mut self) -> &Path {
        self.parent
            .get_or_insert_with(|| source::default_parent(&horizon_core::dir_search::expand_tilde("~")))
    }

    /// Where `remote` would be cloned; looked up again only when the link or the folder changes.
    fn plan(&mut self, remote: &Remote) -> &Plan {
        let parent = self.parent().to_owned();
        if self
            .plan
            .as_ref()
            .is_some_and(|plan| plan.url != remote.url || plan.parent != parent)
        {
            self.plan = None;
        }
        self.plan.get_or_insert_with(|| Plan {
            url: remote.url.clone(),
            existing: source::existing(&parent, remote),
            destination: source::destination(&parent, remote),
            parent,
        })
    }

    pub fn set_parent(&mut self, path: &Path) {
        self.parent = Some(path.to_owned());
    }

    /// Whether the folder picker was choosing the clone folder; that ends its errand.
    pub fn take_choosing_parent(&mut self) -> bool {
        std::mem::take(&mut self.choosing_parent)
    }

    /// The folder clones go into, for seeding the picker.
    pub fn clone_parent(&mut self) -> PathBuf {
        self.parent().to_owned()
    }

    /// Clones the link, or takes a checkout that is already there.
    pub fn start(&mut self, ctx: &Context) {
        if self.job.is_some() {
            return;
        }
        let Some(remote) = self.remote().cloned() else {
            return;
        };
        let parent = self.parent().to_owned();
        if let Some(path) = source::existing(&parent, &remote) {
            self.ready = Some(path);
            return;
        }
        let destination = source::destination(&parent, &remote);
        // What the probe read without one needs no token, whatever was typed before.
        let token = (!self.public).then(|| Token::new(&remote, &self.token)).flatten();
        self.token_tried = token.is_some();
        self.token_focused = false;
        let remember = token.is_some() && self.remember;
        let (sender, receiver) = channel();
        let job = Job {
            receiver,
            cancel: Cancellation::default(),
            progress: Progress::default(),
        };
        let (cancel, progress, ctx) = (job.cancel.clone(), job.progress.clone(), ctx.clone());
        self.failure = None;
        self.note = None;
        self.job = Some(job);
        std::thread::spawn(move || {
            let result = source::clone(&remote, &destination, token.as_ref(), &cancel, &progress).map(|()| {
                let kept = token
                    .as_ref()
                    .filter(|_| remember && !cancel.is_cancelled())
                    .map(source::remember);
                Cloned {
                    path: destination,
                    note: (kept == Some(false))
                        .then_some("Cloned. Git has no credential helper to keep the token, so it was not saved."),
                }
            });
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }

    /// The checkout once the clone finished, or at once for one already on disk.
    pub fn poll(&mut self, ctx: &Context) -> Option<PathBuf> {
        if let Some(path) = self.ready.take() {
            return Some(path);
        }
        let job = self.job.as_ref()?;
        match job.receiver.try_recv() {
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                None
            }
            Ok(Ok(Cloned { path, note })) => {
                self.finish();
                self.note = note;
                Some(path)
            }
            Ok(Err(failure)) => {
                self.finish();
                // Asked for a sign-in after all: what the probe read no longer counts.
                if matches!(failure, Failure::SignIn(_)) {
                    self.public = false;
                }
                self.failure = Some(failure);
                None
            }
            Err(TryRecvError::Disconnected) => {
                self.finish();
                self.failure = Some(Failure::Other("The clone stopped unexpectedly.".into()));
                None
            }
        }
    }

    /// Looks up, once the link has settled, whether it can be read without a token.
    fn probe_access(&mut self, ctx: &Context, remote: &Remote) {
        if let Some(receiver) = &self.probe {
            match receiver.try_recv() {
                Ok(Ok(())) => self.public = true,
                Ok(Err(failure)) => self.failure = Some(failure),
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(100));
                    return;
                }
                Err(TryRecvError::Disconnected) => {}
            }
            self.probe = None;
            return;
        }
        if self.probed.as_deref() == Some(remote.url.as_str()) || self.plan(remote).existing.is_some() {
            return;
        }
        let quiet = self.changed.map_or(SETTLE, |at| at.elapsed());
        if quiet < SETTLE {
            ctx.request_repaint_after(SETTLE.saturating_sub(quiet));
            return;
        }
        self.probed = Some(remote.url.clone());
        let (sender, receiver) = channel();
        self.probe_cancel = Cancellation::default();
        let (remote, ctx, cancel) = (remote.clone(), ctx.clone(), self.probe_cancel.clone());
        std::thread::spawn(move || {
            let _ = sender.send(source::probe(&remote, None, &cancel));
            ctx.request_repaint();
        });
        self.probe = Some(receiver);
    }

    fn finish(&mut self) {
        self.job = None;
        self.token.zeroize();
    }

    /// What Continue would do now, or what is missing before it can.
    pub fn next_step(&mut self) -> Result<&'static str, &'static str> {
        if self.job.is_some() {
            return Err("Cloning…");
        }
        let Some(remote) = self.remote().cloned() else {
            return Err("Paste a GitHub or GitLab link, or choose a folder.");
        };
        // Nothing is cloned before the link has been looked up, unless it is already on disk.
        let looked_up = self.probed.as_deref() == Some(remote.url.as_str()) && self.probe.is_none();
        if !looked_up && self.plan(&remote).existing.is_none() {
            return Err("Checking access…");
        }
        if matches!(self.failure, Some(Failure::SignIn(_))) && self.token.trim().is_empty() {
            return Err("Paste a token with read access to continue.");
        }
        Ok("Continue clones the repository, then you choose where it runs.")
    }
}

/// `scheme://host[:port]` of a repository address: the part a token is good for.
fn origin(url: &str) -> &str {
    url.match_indices('/').nth(2).map_or(url, |(end, _)| &url[..end])
}

impl Drop for State {
    /// Closing the dialog ends a running clone and forgets a token that was never used.
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.cancel();
        }
        self.probe_cancel.cancel();
        self.token.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> State {
        let mut state = State::default();
        state.input.push_str(text);
        state
    }

    #[test]
    fn continue_says_what_is_missing() {
        assert_eq!(
            typed("").next_step(),
            Err("Paste a GitHub or GitLab link, or choose a folder.")
        );
        let mut link = typed("github.com/demo-org/demo-atlas");
        assert_eq!(
            link.next_step(),
            Err("Checking access…"),
            "not before the link is looked up"
        );
        link.probed = link.remote().map(|remote| remote.url.clone());
        assert!(link.next_step().is_ok());
        link.failure = Some(Failure::SignIn("github.com".into()));
        assert_eq!(link.next_step(), Err("Paste a token with read access to continue."));
        link.token = "token".into();
        assert!(link.next_step().is_ok());
    }

    #[test]
    fn a_link_is_looked_up_only_after_it_stands_still() {
        let mut link = typed("github.com/demo-org/demo-atlas");
        assert!(link.remote().is_some());
        assert!(link.probe.is_none() && link.probed.is_none());
        link.input.push('x');
        link.remote();
        assert!(
            link.changed.is_some_and(|at| at.elapsed() < SETTLE),
            "typing restarts the wait"
        );
    }

    #[test]
    fn the_field_mirrors_a_repository_chosen_elsewhere_and_notices_edits() {
        let mut state = State::default();
        state.mirror("/tmp/demo/atlas");
        assert!(state.input.ends_with("atlas") && !state.editing());
        state.input = "github.com/demo-org/other".into();
        assert!(state.editing());
        state.mirror("/tmp/demo/atlas");
        assert!(
            state.editing(),
            "the same repository does not overwrite what is being typed"
        );
        state.mirror("/tmp/demo/other");
        assert!(!state.editing(), "a new repository replaces the text");
        state.input.clear();
        assert!(state.editing(), "clearing the field leaves the old repository behind");
    }

    #[test]
    fn a_checkout_that_is_already_there_is_the_plan() {
        let temp = tempfile::tempdir().unwrap();
        let remote = source::parse("github.com/demo-org/demo-atlas").unwrap();
        let checkout = temp.path().join("demo-atlas");
        std::fs::create_dir(&checkout).unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&checkout)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "{args:?}");
        };
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "first"]);
        git(&["remote", "add", "origin", &remote.url]);
        let mut state = State::default();
        state.set_parent(temp.path());
        let plan = state.plan(&remote);
        assert_eq!(plan.existing.as_deref(), Some(checkout.as_path()));
        assert_eq!(
            plan.destination,
            temp.path().join("demo-atlas-2"),
            "a fresh clone would go beside it"
        );
    }

    #[test]
    fn a_path_written_another_way_is_still_the_repository_chosen() {
        let mut state = State::default();
        state.mirror("/tmp/demo/atlas");
        state.input = "/tmp/demo/atlas/".into();
        assert!(state.editing());
        state.accept_text();
        assert!(!state.editing());
    }

    #[test]
    fn a_token_never_follows_the_link_to_another_host() {
        let mut state = typed("github.com/demo-org/demo-atlas");
        assert!(state.remote().is_some());
        state.token.push_str("ghp_demo");
        state.token_tried = true;
        state.input = "github.com/demo-org/other".into();
        state.remote();
        assert_eq!(state.token, "ghp_demo", "the same host keeps its token");
        state.input = "gitlab.example.org/group/other".into();
        state.remote();
        assert!(
            state.token.is_empty() && !state.token_tried,
            "another host starts clean"
        );
    }

    #[test]
    fn a_clone_that_asks_for_a_sign_in_after_an_open_probe_takes_the_token() {
        let mut state = State::default();
        state.public = true;
        let (sender, receiver) = channel();
        state.job = Some(Job {
            receiver,
            cancel: Cancellation::default(),
            progress: Progress::default(),
        });
        sender.send(Err(Failure::SignIn("github.com".into()))).unwrap();
        assert_eq!(state.poll(&Context::default()), None);
        assert!(!state.public, "tokens are sent again");
        assert_eq!(state.failure, Some(Failure::SignIn("github.com".into())));
    }

    #[test]
    fn closing_the_dialog_ends_a_running_clone() {
        let cancel = Cancellation::default();
        let mut state = State::default();
        let (_sender, receiver) = channel();
        state.job = Some(Job {
            receiver,
            cancel: cancel.clone(),
            progress: Progress::default(),
        });
        drop(state);
        assert!(cancel.is_cancelled());
    }
}

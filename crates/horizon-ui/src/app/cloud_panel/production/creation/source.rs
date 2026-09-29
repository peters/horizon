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

struct Job {
    receiver: Receiver<Result<PathBuf, Failure>>,
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

    #[cfg(test)]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// The text differs from the repository chosen, so another one is being asked for.
    pub fn editing(&self) -> bool {
        !self.input.trim().is_empty() && self.input.trim() != self.mirrored.trim()
    }

    /// The repository a pasted link names, unless the text is a folder that exists.
    fn remote(&mut self) -> Option<&Remote> {
        if self.parsed_for != self.input {
            self.parsed_for.clone_from(&self.input);
            let path = horizon_core::dir_search::expand_tilde(self.input.trim());
            let before = self.remote.as_ref().map(|remote| remote.host.clone());
            self.remote = source::parse(&self.input).filter(|_| !path.exists());
            // A token was pasted for one host and is never carried to another.
            if before != self.remote.as_ref().map(|remote| remote.host.clone()) {
                self.token.zeroize();
                self.token_tried = false;
                self.token_focused = false;
            }
            self.folder = (!self.input.trim().is_empty() && path.join(".git").exists()).then_some(path);
            self.failure = None;
            self.public = false;
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
        let token = Token::new(&remote.host, &self.token);
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
        self.job = Some(job);
        std::thread::spawn(move || {
            let result = source::clone(&remote, &destination, token.as_ref(), &cancel, &progress).map(|()| {
                if let Some(token) = token.as_ref().filter(|_| remember && !cancel.is_cancelled()) {
                    let _ = source::remember(&remote, token);
                }
                destination
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
            Ok(Ok(path)) => {
                self.finish();
                Some(path)
            }
            Ok(Err(failure)) => {
                self.finish();
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
        let (remote, ctx) = (remote.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = sender.send(source::probe(&remote, None));
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
        if self.remote().is_none() {
            return Err("Paste a GitHub or GitLab link, or choose a folder.");
        }
        if self.probe.is_some() {
            return Err("Checking access…");
        }
        if matches!(self.failure, Some(Failure::SignIn(_))) && self.token.trim().is_empty() {
            return Err("Paste a token with read access to continue.");
        }
        Ok("Continue clones the repository, then you choose where it runs.")
    }
}

impl Drop for State {
    /// Closing the dialog ends a running clone and forgets a token that was never used.
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.cancel();
        }
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

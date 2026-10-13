//! **Use this setup**. Closing the agent's panel only hangs up, so a thread first ends
//! the agent and everything in its process group, killed if they ignore the hang-up,
//! and reads the files outside `.horizon`. The same thread then reads the setup once
//! more, checks it against the proof that passed, checks that the status still asks for
//! the proof and desktop preview that ran and that the preview still answers, and
//! commits it.
//! The dialog keeps drawing meanwhile and only applies what the thread reports.
use super::agent::AgentProcess;
use super::commit::{self, Outside};
use super::set_configuration;
use crate::app::cloud_panel::production::HorizonApp;
use horizon_core::cloud_runtime::repository::launch::Configuration;
use std::path::PathBuf;
use std::time::Duration;

/// What the thread of Use reports.
enum Outcome {
    /// The agent's processes did not all end: its process goes back to the run.
    StillEnding(Option<AgentProcess>),
    /// Use committed nothing, for the reason the dialog shows.
    Refused {
        message: String,
        /// The setup is no longer the proved one, or the folder is another: the run stops.
        stop: bool,
        /// The reading that named changes, the baseline from now on.
        baseline: Option<Outside>,
        /// The ignored files past the reading's limit were named.
        told: bool,
    },
    /// `.horizon` is committed.
    Committed(commit::Committed),
}

impl Outcome {
    fn refused(message: impl Into<String>) -> Self {
        Self::Refused {
            message: message.into(),
            stop: false,
            baseline: None,
            told: false,
        }
    }
}

/// What the thread of Use needs of the run, copied when Use is pressed.
struct Accepting {
    repository: PathBuf,
    identity: super::untrusted::Identity,
    status_path: PathBuf,
    baseline: Outside,
    told: bool,
    finished: bool,
    proved: Option<[u8; 32]>,
    spec: Option<super::proof::Spec>,
    message: &'static str,
    /// The proof and the desktop preview the status asked for, and the loopback port the
    /// preview is live on.
    asked: super::run::Asked,
    preview_port: Option<u16>,
}

/// The thread of Use and what it reports. It holds the agent's process while it runs,
/// so whatever lets the run go waits for it first.
pub(super) struct Using {
    report: std::sync::mpsc::Receiver<Outcome>,
    thread: std::thread::JoinHandle<()>,
}

impl Using {
    /// Waits for the thread to end; the agent's process it handed back, when its
    /// processes did not all end.
    pub(super) fn wait(self) -> Option<AgentProcess> {
        let _ = self.thread.join();
        match self.report.try_recv() {
            Ok(Outcome::StillEnding(agent)) => agent,
            _ => None,
        }
    }
}

impl HorizonApp {
    /// Starts Use: closes the agent's panel and leaves the rest to a thread.
    pub(super) fn use_cloud_repository_setup(&mut self, ctx: &egui::Context) {
        let Some(run) = self.cloud_prototype.production.setup_run.as_mut() else {
            return;
        };
        if run.ending() {
            return;
        }
        // A desktop preview that stopped since the dialog drew is not committed as live,
        // and its agent keeps running, so it hears why and can start it again.
        if !run.preview_live_or_none() {
            self.cloud_prototype.error = Some(PREVIEW_NOT_LIVE.into());
            return;
        }
        let panel = run.panel;
        if self.board.panel(panel).is_some() {
            self.close_panel(panel);
        }
        let Some(run) = self.cloud_prototype.production.setup_run.as_mut() else {
            return;
        };
        let agent = run.agent_process.take();
        let accepting = Accepting {
            repository: run.repository.clone(),
            identity: run.identity,
            status_path: run.status_path.clone(),
            baseline: run.outside.clone(),
            told: run.ignored_partial_told,
            finished: run.finished(),
            proved: run.proof.as_ref().and_then(|proof| proof.inputs),
            spec: run.proof_spec().cloned(),
            message: run.mode.commit_message(),
            asked: run.asked(),
            preview_port: run.named_preview_port(),
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let wake = ctx.clone();
        let thread = std::thread::spawn(move || {
            let outcome = if agent.as_ref().is_none_or(AgentProcess::end) {
                // Read only once nothing of the agent's runs, so no late write races it.
                let outside = commit::outside(&accepting.repository);
                accept(&accepting, outside)
            } else {
                Outcome::StillEnding(agent)
            };
            let _ = sender.send(outcome);
            wake.request_repaint();
        });
        run.ending = Some(Using {
            report: receiver,
            thread,
        });
        self.cloud_prototype.error = None;
    }

    /// Applies what the thread of Use reported. Returns whether Use finished or refused
    /// this frame, so nothing else of the run is synced after it.
    pub(super) fn poll_cloud_setup_use(&mut self, ctx: &egui::Context) -> bool {
        let Some(run) = self.cloud_prototype.production.setup_run.as_mut() else {
            return false;
        };
        let Some(ending) = &run.ending else {
            return false;
        };
        let outcome = match ending.report.try_recv() {
            Ok(outcome) => outcome,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                return false;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                run.ending = None;
                self.cloud_prototype.error = Some("Use this setup stopped before it finished. Press it again.".into());
                return false;
            }
        };
        run.ending = None;
        match outcome {
            Outcome::StillEnding(agent) => {
                run.agent_process = agent;
                self.cloud_prototype.error =
                    Some("The setup agent is still ending. Press Use this setup again in a moment.".into());
                false
            }
            Outcome::Refused {
                message,
                stop,
                baseline,
                told,
            } => {
                run.stopped |= stop;
                if let Some(baseline) = baseline {
                    run.outside = baseline;
                }
                run.ignored_partial_told |= told;
                self.cloud_prototype.error = Some(message);
                true
            }
            Outcome::Committed(committed) => {
                if let Some(mut run) = self.cloud_prototype.production.setup_run.take() {
                    // The live desktop and the GitHub access panel go with the run.
                    for helper in [run.preview_panel.take(), run.fix_panel.take()].into_iter().flatten() {
                        if self.board.panel(helper).is_some() {
                            self.close_panel(helper);
                        }
                    }
                    run.discard();
                }
                // The setup was made and committed on HEAD, so the dialog reads HEAD again.
                self.cloud_prototype.production.revision.clear();
                set_configuration(&mut self.cloud_prototype.production, Configuration::Committed);
                self.read_cloud_profiles(ctx);
                if let commit::Committed::IndexBehind(error) = committed {
                    self.cloud_prototype.error = Some(format!(
                        "Committed .horizon, but your git index was not updated ({error}). Run `git reset -q -- .horizon` in the repository to refresh it."
                    ));
                }
                true
            }
        }
    }
}

/// Commits the files the agent wrote, on the thread of Use, once the agent has ended: the
/// setup is read once, must be the proved one, and is what is committed; files outside
/// `.horizon` that changed are named first.
fn accept(accepting: &Accepting, outside: Outside) -> Outcome {
    // The status as the ended agent left it: a proof or preview it asked for just before
    // Use ended it never ran, so it must still ask for the ones proved and shown.
    if super::run::asked_otherwise_in(&accepting.status_path, &accepting.asked) {
        return Outcome::refused(ASKED_OTHERWISE);
    }
    // The app of a live desktop preview can stop while Use runs.
    if accepting
        .preview_port
        .is_some_and(|port| !super::preview::answers(port))
    {
        return Outcome::refused(PREVIEW_STOPPED);
    }
    let setup = super::snapshot::Setup::read(&accepting.repository, accepting.identity);
    let current = setup
        .as_ref()
        .ok()
        .zip(accepting.spec.as_ref())
        .map(|(setup, spec)| setup.fingerprint(spec));
    let setup = match setup {
        Ok(setup) if accepting.finished && current.is_some() && current == accepting.proved => setup,
        _ => {
            return Outcome::Refused {
                message: "The .horizon files changed after Horizon's proof, so they are not committed. Start the setup again to prove them.".into(),
                stop: true,
                baseline: None,
                told: false,
            };
        }
    };
    // A reading that stopped at a limit could not name every change, so nothing is
    // committed on it.
    if let Some(reason) = outside.incomplete().or(accepting.baseline.incomplete()) {
        return Outcome::refused(format!(
            "Nothing was committed: Horizon cannot check every file outside .horizon, because {reason}. Commit .horizon yourself after reviewing it."
        ));
    }
    // Ignored files past their limit are not compared: named once, as changes are, and
    // committed only when the person presses Use again knowing that.
    if (accepting.baseline.ignored_partial() || outside.ignored_partial()) && !accepting.told {
        return Outcome::Refused {
            message: IGNORED_PARTIAL.into(),
            stop: false,
            baseline: None,
            told: true,
        };
    }
    // The agent may change only .horizon. Anything else that changed during the setup
    // is named once, every path in a report beside the run's status; pressing Use again
    // commits .horizon and leaves the rest. Only a complete report lets the next press
    // commit: without one, the baseline stays and the changes are named again.
    let changed = commit::changed(&accepting.baseline, &outside);
    if !changed.is_empty() {
        let report = accepting.status_path.with_file_name("outside-changes.txt");
        let listed = super::proof::create_private(&report)
            .and_then(|mut file| std::io::Write::write_all(&mut file, (changed.join("\n") + "\n").as_bytes()));
        return match listed {
            Ok(()) => Outcome::Refused {
                message: outside_changes_message(&changed, &report),
                stop: false,
                baseline: Some(outside),
                told: false,
            },
            Err(error) => Outcome::refused(format!(
                "Nothing was committed: files outside .horizon changed while the setup ran, and Horizon could not list them all in {} ({error}). Stop the setup and review the repository.",
                report.display()
            )),
        };
    }
    // Git works by path: the repository folder must still be the run's when it does.
    if super::untrusted::identity(&accepting.repository) != accepting.identity {
        return Outcome::Refused {
            message: format!("Nothing was committed: {}.", super::untrusted::OTHER_REPOSITORY),
            stop: true,
            baseline: None,
            told: false,
        };
    }
    match commit::commit(&accepting.repository, accepting.message, &setup) {
        Ok(committed) => Outcome::Committed(committed),
        Err(error) => Outcome::refused(format!("Could not commit .horizon: {error}")),
    }
}

/// What Use says when the desktop preview the agent named is not live as it is pressed.
const PREVIEW_NOT_LIVE: &str = "The desktop preview stopped, so nothing is committed. The agent hears why; Use this setup returns once the preview is live again.";

/// What Use says when the agent changed its proof or desktop preview just before Use
/// ended it.
const ASKED_OTHERWISE: &str = "The agent changed the proof or the desktop preview in its status file as Use ended it, so what it asks for last was never proved or shown and nothing is committed. Start the setup again, or Stop.";

/// What Use says when the desktop preview stopped answering while Use ran.
const PREVIEW_STOPPED: &str =
    "The desktop preview stopped answering while Use ran, so nothing is committed. Start the setup again, or Stop.";

/// What Use says before it commits when the repository has more ignored files than a
/// reading compares.
const IGNORED_PARTIAL: &str = "This repository has more than 100,000 ignored files, such as build output or dependencies, and Horizon compared only the first 100,000 for changes during the setup. Review the others yourself if the agent could have changed them, then press Use this setup again to commit .horizon.";

/// What Use says when files outside `.horizon` changed while the setup ran: the first
/// few, and where the report that lists every one is.
fn outside_changes_message(changed: &[String], report: &std::path::Path) -> String {
    let shown = changed.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
    let listed = format!(", all listed in {}", report.display());
    let more = match changed.len().saturating_sub(5) {
        0 => listed,
        more => format!(" and {more} more{listed}"),
    };
    format!(
        "Changed outside .horizon while the setup ran: {shown}{more}. Use this setup commits only .horizon on the current HEAD and leaves the rest as it is. Review the changes, then press Use this setup again."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_changed_path_is_named_or_in_the_report() {
        let changed: Vec<String> = (1..=7).map(|index| format!("src/file{index}.rs")).collect();
        let report = std::path::Path::new("/run/outside-changes.txt");
        let message = outside_changes_message(&changed, report);
        assert!(
            message.contains("src/file5.rs and 2 more, all listed in /run/outside-changes.txt"),
            "{message}"
        );
        let few = outside_changes_message(&changed[..2], report);
        assert!(few.contains("src/file2.rs, all listed in"), "{few}");
    }
}

#[cfg(all(test, unix))]
mod blocking_tests {
    use super::super::Mode;
    use super::super::run::Run;
    use std::process::Command;
    use std::time::Instant;

    #[test]
    fn use_returns_at_once_while_git_waits_on_the_repository() {
        let (_temp, ctx, mut app) =
            crate::app::test_support::test_app_with_startup(horizon_core::StartupDecision::Ephemeral {
                runtime_state: Box::default(),
            });
        let repository = tempfile::tempdir().unwrap();
        let path = repository.path().to_path_buf();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&path)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-q"]);
        // Every git command now waits until someone opens the pipe for writing.
        let fifo = repository.path().join("waits");
        assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let config = repository.path().join(".git/config");
        let plain = std::fs::read_to_string(&config).unwrap();
        std::fs::write(&config, format!("{plain}[include]\n\tpath = {}\n", fifo.display())).unwrap();
        let status = tempfile::tempdir().unwrap();
        let run = Run::new(
            horizon_core::PanelId(1),
            horizon_core::PanelKind::Usage,
            path.clone(),
            status.path().join("status.json"),
            Mode::New,
        );
        app.cloud_prototype.production.setup_run = Some(run);
        let (signal, signalled) = std::sync::mpsc::channel::<()>();
        let opener = std::thread::spawn(move || {
            let _ = signalled.recv_timeout(std::time::Duration::from_secs(10));
            // The include goes first, so the git commands after this one wait for nothing.
            std::fs::write(&config, plain).unwrap();
            // Then the git command that waits on the pipe gets its end: opened without
            // waiting, which fails until a reader has it open, and closed at once.
            let deadline = Instant::now() + std::time::Duration::from_secs(5);
            while Instant::now() < deadline {
                use std::os::unix::fs::OpenOptionsExt as _;
                let opened = std::fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed())
                    .open(&fifo);
                if opened.is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
        let started = Instant::now();
        app.use_cloud_repository_setup(&ctx);
        let finished = app.poll_cloud_setup_use(&ctx);
        let took = started.elapsed();
        let _ = signal.send(());
        assert!(took < std::time::Duration::from_secs(2), "the frame waited {took:?}");
        assert!(!finished, "Use has not reported yet");
        assert!(
            app.cloud_prototype
                .production
                .setup_run
                .as_ref()
                .is_some_and(Run::ending),
            "the dialog shows Use at work"
        );
        // Stop cannot cancel Use once it started, so it does nothing until Use reported.
        app.cloud_repository_setup_action(&ctx, super::super::run::Action::Stop);
        assert!(
            app.cloud_prototype.production.setup_run.is_some(),
            "Stop leaves the run to Use"
        );
        opener.join().unwrap();
        // Then it reports: this run has no proof that passed, so nothing is committed.
        let deadline = Instant::now() + std::time::Duration::from_secs(60);
        while !app.poll_cloud_setup_use(&ctx) {
            assert!(Instant::now() < deadline, "Use reported in time");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            app.cloud_prototype
                .error
                .as_deref()
                .is_some_and(|error| error.contains("not committed"))
        );
    }

    #[test]
    fn use_returns_at_once_while_the_preview_desktop_does_not_answer() {
        let (_temp, ctx, mut app) =
            crate::app::test_support::test_app_with_startup(horizon_core::StartupDecision::Ephemeral {
                runtime_state: Box::default(),
            });
        let repository = tempfile::tempdir().unwrap();
        let status = tempfile::tempdir().unwrap();
        // A desktop that takes the connection but never greets: a check of it waits.
        let desktop = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = desktop.local_addr().unwrap().port();
        let mut run = Run::new(
            horizon_core::PanelId(1),
            horizon_core::PanelKind::Usage,
            repository.path().to_path_buf(),
            status.path().join("status.json"),
            Mode::New,
        );
        run.show_live_preview(port);
        app.cloud_prototype.production.setup_run = Some(run);
        let (signal, signalled) = std::sync::mpsc::channel::<()>();
        let closer = std::thread::spawn(move || {
            let _ = signalled.recv_timeout(std::time::Duration::from_secs(10));
            // The desktop goes, and the connection waiting for its greeting with it.
            drop(desktop);
        });
        let started = Instant::now();
        app.use_cloud_repository_setup(&ctx);
        let finished = app.poll_cloud_setup_use(&ctx);
        let took = started.elapsed();
        let _ = signal.send(());
        assert!(took < std::time::Duration::from_secs(2), "the frame waited {took:?}");
        assert!(!finished, "Use has not reported yet");
        assert!(
            app.cloud_prototype
                .production
                .setup_run
                .as_ref()
                .is_some_and(Run::ending),
            "the dialog shows Use at work"
        );
        closer.join().unwrap();
        // Then it reports: the desktop did not answer, so nothing is committed.
        let deadline = Instant::now() + std::time::Duration::from_secs(60);
        while !app.poll_cloud_setup_use(&ctx) {
            assert!(Instant::now() < deadline, "Use reported in time");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(app.cloud_prototype.error.as_deref(), Some(super::PREVIEW_STOPPED));
    }
}

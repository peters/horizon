//! The work of native tests that runs off the UI thread: the ledger on disk, the check of a
//! request, the looks at the test cloud and the delivery of reports to the Upkeep worker.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Instant;

use horizon_core::cloud_runtime::{self, Cancellation, command::Runner, settings::Settings, ssh::Connection};
use horizon_core::upkeep::native::{self, Report, Request, State as Test, probe};
use horizon_core::upkeep::worker;

use super::RETRY;
use crate::app::upkeep::{Native, Probed};

/// Reads the ledger the first time. Returns whether it is there.
pub(super) fn load(ctx: &egui::Context, cloud_id: &str, native: &mut Native, now: Instant) -> bool {
    if native.ledger.is_some() {
        return true;
    }
    if let Some(rx) = &native.loading {
        match rx.try_recv() {
            Ok(Ok(ledger)) => {
                native.loading = None;
                native.ledger = Some(ledger);
                native.intake.get_or_insert_with(channel);
                // The worker may hold reports of an earlier run of Horizon.
                native.undelivered = true;
                return true;
            }
            Ok(Err(error)) => {
                // A ledger that cannot be read may name a test cloud: it is never replaced.
                tracing::warn!(cloud = %cloud_id, %error, "could not read the native test ledger");
                native.loading = None;
                native.load_after = Some(now + RETRY);
            }
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => native.loading = None,
        }
    }
    if let Some(after) = native.load_after.filter(|after| now < *after) {
        ctx.request_repaint_after(after - now);
        return false;
    }
    let (tx, rx) = channel();
    let (repaint, cloud) = (ctx.clone(), cloud_id.to_owned());
    std::thread::spawn(move || {
        let _ = tx.send(native::load(&cloud));
        repaint.request_repaint();
    });
    native.loading = Some(rx);
    false
}

/// Saves the ledger when it changed and no save is under way, and again a while after a save
/// failed. Returns whether the ledger on disk is the one in memory, or `None` while a save of
/// it is under way.
pub(super) fn save(
    ctx: &egui::Context,
    cloud_id: &str,
    ledger: &native::Ledger,
    native: &mut Native,
    now: Instant,
) -> Option<bool> {
    if let Some(rx) = &native.saving {
        match rx.try_recv() {
            Err(TryRecvError::Empty) => return None,
            Ok(stored) => native.stored = stored,
            Err(TryRecvError::Disconnected) => native.stored = false,
        }
        native.saving = None;
        if !native.stored {
            // The disk keeps an older ledger, which a restart would read: save again.
            native.dirty = true;
            native.save_after = Some(now + RETRY);
        }
    }
    if let Some(after) = native.save_after.filter(|after| now < *after) {
        ctx.request_repaint_after(after - now);
        return Some(false);
    }
    if !native.dirty {
        return Some(native.stored);
    }
    native.dirty = false;
    // Every change of the ledger goes to the worker too.
    native.undelivered = true;
    let (tx, rx) = channel();
    let (repaint, cloud, ledger) = (ctx.clone(), cloud_id.to_owned(), ledger.clone());
    std::thread::spawn(move || {
        let stored = native::save(&cloud, &ledger);
        if let Err(error) = &stored {
            tracing::warn!(cloud = %cloud, %error, "could not save the native test ledger");
        }
        let _ = tx.send(stored.is_ok());
        repaint.request_repaint();
    });
    native.saving = Some(rx);
    None
}

/// Checks `request` on a thread and, when Horizon may test it, marks a new cloud id as a
/// cloud for untrusted code before anything of it exists.
pub(super) fn approve(
    ctx: &egui::Context,
    root: &Path,
    upkeep: &str,
    request: Request,
) -> Receiver<Result<(native::Approved, String), String>> {
    let (tx, rx) = channel();
    let (repaint, root, upkeep) = (ctx.clone(), root.to_path_buf(), upkeep.to_owned());
    std::thread::spawn(move || {
        // The settings saved now: the request may have waited while the person changed them.
        let config = worker::load(&upkeep).map_err(|error| format!("Upkeep could not read its settings: {error}."));
        let approved = config
            .and_then(|config| native::approve(&root, &request, &config, &Cancellation::default()))
            .and_then(|approved| {
                let cloud = cloud_runtime::new_id();
                cloud_runtime::state::cloud_directory(&root, &cloud)
                    .map_err(|error| error.to_string())
                    .and_then(|folder| {
                        cloud_runtime::settings::mark_isolated(&folder).map_err(|error| error.to_string())
                    })
                    .map(|()| (approved, cloud))
            });
        let _ = tx.send(approved);
        repaint.request_repaint();
    });
    rx
}

/// The SSH connection to the worker of cloud `cloud`.
fn connect(root: &Path, cloud: &str, state: &cloud_runtime::state::Deployment) -> Result<Connection, String> {
    let settings = Settings::load(&root.join("settings.json")).map_err(|error| error.to_string())?;
    let state_root = cloud_runtime::state::cloud_directory(root, cloud).map_err(|error| error.to_string())?;
    let worker = state.worker.as_ref().ok_or("The cloud has no worker")?;
    Connection::new(worker, &settings, &state_root).map_err(|error| error.to_string())
}

/// Looks at the test cloud on a thread: its result and, once there is one, or at the end of
/// the time limit (`last`), the evidence, which it copies to `evidence`.
pub(super) fn look(
    ctx: &egui::Context,
    root: &Path,
    cloud: String,
    state: cloud_runtime::state::Deployment,
    evidence: PathBuf,
    last: bool,
) -> Receiver<Result<Probed, String>> {
    let (tx, rx) = channel();
    let (repaint, root) = (ctx.clone(), root.to_path_buf());
    std::thread::spawn(move || {
        let probed = (|| {
            let connection = connect(&root, &cloud, &state)?;
            let cancel = Cancellation::default();
            let runner = Runner {
                cancel: &cancel,
                emit: &|_| {},
                secrets: Vec::new(),
            };
            let status = probe::status(&connection, &runner).map_err(|error| error.to_string())?;
            if status.outcome.is_none() && !last {
                return Ok((None, String::new(), Vec::new()));
            }
            // A new test of the same head starts with no evidence of an earlier one.
            let _ = std::fs::remove_dir_all(&evidence);
            let files = probe::collect(&connection, &runner, &status, &evidence).map_err(|error| error.to_string())?;
            Ok((status.outcome, status.report, files))
        })();
        let _ = tx.send(probed);
        repaint.request_repaint();
    });
    rx
}

/// Writes `ledger`'s reports to the Upkeep worker on a thread.
pub(super) fn deliver(
    ctx: &egui::Context,
    root: &Path,
    upkeep: String,
    state: cloud_runtime::state::Deployment,
    ledger: native::Ledger,
) -> Receiver<Result<(), String>> {
    let (tx, rx) = channel();
    let (repaint, root) = (ctx.clone(), root.to_path_buf());
    std::thread::spawn(move || {
        let delivered = connect(&root, &upkeep, &state).and_then(|connection| {
            let cancel = Cancellation::default();
            let runner = Runner {
                cancel: &cancel,
                emit: &|_| {},
                secrets: Vec::new(),
            };
            native::deliver(&connection, &runner, &ledger).map_err(|error| error.to_string())
        });
        let _ = tx.send(delivered);
        repaint.request_repaint();
    });
    rx
}

/// The report that ends a test, from one look at its cloud, or `None` while it continues.
/// Without an outcome, only the `last` look, at the time limit, ends the test, also when it
/// failed.
pub(super) fn finished(request: &Request, probed: Result<Probed, String>, last: bool) -> Option<Report> {
    match probed {
        Ok((Some(outcome), text, files)) => {
            let message = if outcome.summary.is_empty() {
                "The agent on the test cloud wrote no summary.".to_owned()
            } else {
                outcome.summary
            };
            let mut report = Report::new(request, outcome.state, message);
            report.report = text;
            report.files = files;
            Some(report)
        }
        Ok((None, text, files)) if last => {
            let mut report = Report::new(request, Test::Expired, expired_message());
            report.report = text;
            report.files = files;
            Some(report)
        }
        Err(error) if last => Some(Report::new(
            request,
            Test::Expired,
            format!("{} Horizon could not read the test cloud: {error}", expired_message()),
        )),
        Err(error) => {
            tracing::warn!(test = %request.label(), %error, "could not read the native test cloud");
            None
        }
        Ok(_) => None,
    }
}

pub(super) fn expired_message() -> String {
    format!(
        "The test cloud reached its time limit of {} minutes before its agent wrote a result.",
        native::TIME_LIMIT.as_secs() / 60
    )
}

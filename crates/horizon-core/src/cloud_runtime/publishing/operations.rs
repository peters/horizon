//! Bounded host-owned receipts for destination operations that can wait for sign-in.
use super::management::{self, Info, Request};
use crate::cloud_runtime::{Cancellation, Error, Event, Result, command::Runner, github::Prompt};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant, UNIX_EPOCH},
};

const MAX_ACTIVE: usize = 8;
const MAX_RETAINED: usize = 64;
const RETENTION: Duration = Duration::from_mins(5);
const OBSERVATION: Duration = Duration::from_secs(30);
const LIFETIME: Duration = Duration::from_mins(15);
const UNKNOWN: &str = "Unknown or expired image-destination operation; list saved state before retrying";

/// Supplied by the trusted host after checking the live local agent panel.
/// These fields are never accepted as agent-supplied authority.
/// Direct dispatch/start require the exact canonical Git root. Launch-directory
/// dispatch retains the registered launch path here and resolves its checkout in
/// the shared worker. Public callers cannot supply either source path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub actor: String,
    pub host: String,
    pub root: PathBuf,
    pub repository: PathBuf,
}

/// A live local panel's registered launch directory, supplied only by its host.
/// The shared worker resolves its Git root before reading or changing a choice.
#[derive(Clone, Debug)]
pub struct LaunchOwner {
    pub actor: String,
    pub host: String,
    pub root: PathBuf,
    pub directory: PathBuf,
}

impl LaunchOwner {
    fn receipt_scope(self) -> Owner {
        Owner {
            actor: self.actor,
            host: self.host,
            root: self.root,
            repository: self.directory,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SignIn {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_at_unix: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub operation_id: String,
    pub done: bool,
    pub cancel_requested: bool,
    pub sign_in: Option<SignIn>,
    pub result: Option<Info>,
    pub error: Option<String>,
}

enum Report {
    SignIn(SignIn),
    Finished(std::result::Result<Box<Info>, String>),
}

struct Job {
    owner: Owner,
    request: Request,
    receiver: Receiver<Report>,
    cancel: Cancellation,
    status: Status,
    finished: Option<Instant>,
    started: Instant,
    last_observed: Instant,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Job {
    fn poll(&mut self, now: Instant) {
        if self.status.done {
            return;
        }
        if now.saturating_duration_since(self.started) >= LIFETIME
            || (matches!(self.request.action, management::Action::Authenticate { .. })
                && now.saturating_duration_since(self.last_observed) >= OBSERVATION)
        {
            self.cancel.cancel();
            self.status.cancel_requested = true;
        }
        for _ in 0..16 {
            match self.receiver.try_recv() {
                Ok(Report::SignIn(prompt)) => self.status.sign_in = Some(prompt),
                Ok(Report::Finished(result)) => {
                    self.status.sign_in = None;
                    match result {
                        Ok(info) => self.status.result = Some(*info),
                        Err(error) => self.status.error = Some(error),
                    }
                    self.status.done = true;
                    self.finished = Some(now);
                    return;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.status.sign_in = None;
                    self.status.done = true;
                    self.status.error = Some(
                        "Image destination operation ended without a result; list saved state before retrying".into(),
                    );
                    self.finished = Some(now);
                    return;
                }
            }
        }
    }
}

/// Volatile bounded receipts, not durable proof that a mutation did or did not commit.
/// Once a receipt expires or the host restarts, callers must list saved state and
/// review a new request. Every actual settings mutation still uses compare-and-swap.
#[derive(Default)]
pub struct Manager {
    jobs: BTreeMap<String, Job>,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Readback {
    Status {},
    Cancel {},
}

impl Manager {
    /// Decode a nonsecret public action within an already authorized host scope.
    /// # Errors
    /// Invalid action fields, profile scope or receipt ownership.
    pub fn dispatch(
        &mut self,
        owner: Owner,
        id: &str,
        profile: Option<String>,
        action: serde_json::Value,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Status> {
        self.dispatch_with(owner, id, profile, action, notify, false)
    }

    /// Resolve a host-registered launch directory on the worker, including subfolders.
    /// Receipts remain bound to that exact launch path and live panel, not a caller path.
    /// # Errors
    /// As [`Self::dispatch`], or an invalid checkout/configuration on the worker.
    pub fn dispatch_launch(
        &mut self,
        owner: LaunchOwner,
        id: &str,
        profile: Option<String>,
        action: serde_json::Value,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Status> {
        self.dispatch_with(owner.receipt_scope(), id, profile, action, notify, true)
    }

    fn dispatch_with(
        &mut self,
        owner: Owner,
        id: &str,
        profile: Option<String>,
        action: serde_json::Value,
        notify: Arc<dyn Fn() + Send + Sync>,
        launch: bool,
    ) -> Result<Status> {
        if let Ok(readback) = serde_json::from_value::<Readback>(action.clone()) {
            if profile.is_some() {
                return Err(Error::Invalid("Receipt status and cancellation take no profile"));
            }
            return match readback {
                Readback::Status {} => self.status(&owner, id),
                Readback::Cancel {} => self.cancel(&owner, id),
            };
        }
        let action = serde_json::from_value(action).map_err(|_| Error::Invalid("Invalid image-destination action"))?;
        let profile = profile
            .filter(|name| horizon_cloud::valid_id(name))
            .ok_or(Error::Invalid("Choose a named build profile"))?;
        let request = Request {
            repository: owner.repository.clone(),
            profile,
            action,
        };
        if launch {
            let worker_owner = owner.clone();
            let worker_request = request.clone();
            let operation = id.to_owned();
            self.start_with(owner, request, id, notify, move |cancel, emit| {
                management::manage(
                    &worker_owner.root,
                    &worker_request,
                    &operation,
                    &Runner {
                        cancel,
                        emit,
                        secrets: Vec::new(),
                    },
                )
            })
        } else {
            self.start(owner, request, id, notify)
        }
    }

    /// Start or read back exactly the same retained request. Runs all I/O off the caller thread.
    /// # Errors
    /// A changed owner/request, unsafe scope, invalid operation ID or active-operation limit.
    pub fn start(
        &mut self,
        owner: Owner,
        request: Request,
        id: &str,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Status> {
        let operation = id.to_owned();
        let worker_owner = owner.clone();
        let worker_request = request.clone();
        self.start_with(owner, request, id, notify, move |cancel, emit| {
            management::manage_owned(
                &worker_owner.root,
                &worker_request,
                &operation,
                &worker_owner.repository,
                &Runner {
                    cancel,
                    emit,
                    secrets: Vec::new(),
                },
            )
        })
    }

    fn start_with(
        &mut self,
        owner: Owner,
        request: Request,
        id: &str,
        notify: Arc<dyn Fn() + Send + Sync>,
        work: impl FnOnce(&Cancellation, &dyn Fn(Event)) -> Result<Info> + Send + 'static,
    ) -> Result<Status> {
        self.poll(|_| true);
        if !horizon_cloud::valid_id(id)
            || request.repository != owner.repository
            || !owner.root.is_absolute()
            || !owner.repository.is_absolute()
        {
            return Err(Error::Invalid("Invalid image-destination operation scope"));
        }
        if let Some(job) = self.jobs.get_mut(id) {
            if job.owner != owner {
                return Err(Error::Invalid(UNKNOWN));
            }
            if job.request != request {
                return Err(Error::Invalid("This operation ID already names another request"));
            }
            job.last_observed = Instant::now();
            return Ok(job.status.clone());
        }
        if self.jobs.values().filter(|job| !job.status.done).count() >= MAX_ACTIVE
            || self
                .jobs
                .values()
                .any(|job| job.owner.actor == owner.actor && job.owner.host == owner.host && !job.status.done)
        {
            return Err(Error::Invalid(
                "An image-destination operation is still running; poll or cancel it before starting another",
            ));
        }
        if self.jobs.len() >= MAX_RETAINED {
            let oldest = self
                .jobs
                .iter()
                .filter_map(|(id, job)| job.finished.map(|at| (id.clone(), at)))
                .min_by_key(|(_, at)| *at)
                .map(|(id, _)| id);
            if let Some(oldest) = oldest {
                self.jobs.remove(&oldest);
            }
        }
        let cancel = Cancellation::default();
        let worker_cancel = cancel.clone();
        let (send, receiver) = mpsc::sync_channel(16);
        std::thread::spawn(move || {
            let emit = |event| {
                if let Event::GitHub(Prompt::Publish {
                    user_code,
                    verification_uri,
                    expires_at,
                }) = event
                {
                    let prompt = SignIn {
                        user_code,
                        verification_uri,
                        expires_at_unix: expires_at
                            .duration_since(UNIX_EPOCH)
                            .map_or(0, |duration| duration.as_secs()),
                    };
                    if send.send(Report::SignIn(prompt)).is_err() {
                        worker_cancel.cancel();
                    }
                    notify();
                }
            };
            let result = work(&worker_cancel, &emit)
                .map(Box::new)
                .map_err(|error| error.to_string());
            let _ = send.send(Report::Finished(result));
            notify();
        });
        let status = Status {
            operation_id: id.into(),
            done: false,
            cancel_requested: false,
            sign_in: None,
            result: None,
            error: None,
        };
        self.jobs.insert(
            id.into(),
            Job {
                owner,
                request,
                receiver,
                cancel,
                status: status.clone(),
                finished: None,
                started: Instant::now(),
                last_observed: Instant::now(),
            },
        );
        Ok(status)
    }

    /// Whether receipts still need background polling, even without panel output.
    #[must_use]
    pub fn running(&self) -> bool {
        self.jobs.values().any(|job| !job.status.done)
    }

    /// Consume bounded reports and cancel operations whose original host-owned scope vanished.
    pub fn poll(&mut self, active: impl Fn(&Owner) -> bool) {
        self.poll_at(Instant::now(), active);
    }

    fn poll_at(&mut self, now: Instant, active: impl Fn(&Owner) -> bool) {
        for job in self.jobs.values_mut() {
            if !active(&job.owner) && !job.status.done {
                job.cancel.cancel();
                job.status.cancel_requested = true;
            }
            job.poll(now);
        }
        self.jobs.retain(|_, job| {
            job.finished
                .is_none_or(|at| now.saturating_duration_since(at) < RETENTION)
        });
    }

    /// # Errors
    /// An absent receipt or a caller that no longer owns its original scope.
    pub fn status(&mut self, owner: &Owner, id: &str) -> Result<Status> {
        self.status_at(owner, id, Instant::now())
    }

    fn status_at(&mut self, owner: &Owner, id: &str, now: Instant) -> Result<Status> {
        let job = self
            .jobs
            .get_mut(id)
            .filter(|job| {
                job.owner == *owner
                    && job
                        .finished
                        .is_none_or(|at| now.saturating_duration_since(at) < RETENTION)
            })
            .ok_or(Error::Invalid(UNKNOWN))?;
        job.poll(now);
        job.last_observed = now;
        Ok(job.status.clone())
    }

    /// Request cancellation; it is not completion or proof that settings were unchanged.
    /// # Errors
    /// An absent receipt or a caller outside its original scope.
    pub fn cancel(&mut self, owner: &Owner, id: &str) -> Result<Status> {
        self.cancel_at(owner, id, Instant::now())
    }

    fn cancel_at(&mut self, owner: &Owner, id: &str, now: Instant) -> Result<Status> {
        let job = self
            .jobs
            .get_mut(id)
            .filter(|job| {
                job.owner == *owner
                    && job
                        .finished
                        .is_none_or(|at| now.saturating_duration_since(at) < RETENTION)
            })
            .ok_or(Error::Invalid(UNKNOWN))?;
        if !job.status.done {
            job.cancel.cancel();
            job.status.cancel_requested = true;
        }
        job.poll(now);
        job.last_observed = now;
        Ok(job.status.clone())
    }
}

#[cfg(test)]
mod tests;

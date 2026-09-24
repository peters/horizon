//! Discovery jobs report their stage so a stall is attributed to the credential
//! store, the provider request or catalog decoding, each with its own budget.
use super::CatalogError;
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Credential-store reads (for example an OS keychain prompt) cannot be cancelled.
pub(super) const CREDENTIAL_BUDGET: Duration = Duration::from_secs(10);
/// Global HTTP budget covering connection, headers and the bounded body.
pub(super) const REQUEST_BUDGET: Duration = Duration::from_secs(10);
/// Lets the transport report its own timeout before the consumer declares a stall.
const REQUEST_GRACE: Duration = Duration::from_secs(2);
/// Decoding is CPU-bound over at most 8 MiB and 50,000 rows.
const DECODE_BUDGET: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogStage {
    Credentials,
    Request,
    Decode,
}

impl CatalogStage {
    const fn budget(self) -> Duration {
        match self {
            Self::Credentials => CREDENTIAL_BUDGET,
            Self::Request => REQUEST_BUDGET.saturating_add(REQUEST_GRACE),
            Self::Decode => DECODE_BUDGET,
        }
    }

    const fn stalled(self) -> CatalogError {
        match self {
            Self::Credentials => CatalogError::CredentialsTimedOut,
            Self::Request | Self::Decode => CatalogError::Unavailable,
        }
    }
}

struct JobCell {
    stage: Mutex<(CatalogStage, Instant)>,
    running: AtomicBool,
    /// The owning cache's clock offset when the job started (zero outside tests).
    skew: Duration,
}

/// Owned by exactly one discovery job. Dropping it, including during a panic
/// or when the job never starts, marks the job as finished.
pub struct CatalogProgress(Arc<JobCell>);

impl CatalogProgress {
    /// Entering a stage restarts that stage's budget.
    pub fn enter(&self, stage: CatalogStage) {
        *self.0.stage.lock().unwrap_or_else(PoisonError::into_inner) = (stage, Instant::now() + self.0.skew);
    }
}

impl Drop for CatalogProgress {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::Release);
    }
}

/// The cache's view of a job that may outlive its consumer.
#[derive(Clone)]
pub(super) struct Job(Arc<JobCell>);

impl Job {
    pub(super) fn start(skew: Duration) -> (Self, CatalogProgress) {
        let cell = Arc::new(JobCell {
            stage: Mutex::new((CatalogStage::Credentials, Instant::now() + skew)),
            running: AtomicBool::new(true),
            skew,
        });
        (Self(Arc::clone(&cell)), CatalogProgress(cell))
    }

    pub(super) fn running(&self) -> bool {
        self.0.running.load(Ordering::Acquire)
    }

    pub(super) fn stage(&self) -> CatalogStage {
        self.0.stage.lock().unwrap_or_else(PoisonError::into_inner).0
    }

    /// The error a consumer reports once the current stage exceeds its budget.
    pub(super) fn stall(&self, now: Instant) -> Option<CatalogError> {
        let (stage, since) = *self.0.stage.lock().unwrap_or_else(PoisonError::into_inner);
        (self.running() && now.saturating_duration_since(since) >= stage.budget()).then(|| stage.stalled())
    }

    pub(super) fn healthy(&self, now: Instant) -> bool {
        self.running() && self.stall(now).is_none()
    }
}

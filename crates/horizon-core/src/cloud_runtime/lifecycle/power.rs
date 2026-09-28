//! Explicit power requests with their mutation evidence: a pending mutation is
//! recorded only once the provider is about to send the request.
use super::super::{
    Error, Result, Stage,
    mutation::{self, State as Mutation},
    state::{Deployment, Store},
};

/// The provider's power request for a worker, given the hook it calls once its
/// read-only checks pass and immediately before it sends the request.
pub(in crate::cloud_runtime) type Announce<'a> =
    &'a mut dyn FnMut() -> std::result::Result<(), horizon_cloud::CloudError>;

/// Records a pending mutation only when the provider is about to send one, so a
/// failure in its read-only checks is never mistaken for an uncertain outcome.
struct Boundary<'a> {
    observe: mutation::Observer<'a>,
    /// The evidence before this attempt, once the pending mutation was recorded.
    prior: Option<Mutation>,
    /// Why the pending mutation could not be recorded; nothing was sent then.
    refused: Option<Error>,
}

impl<'a> Boundary<'a> {
    fn new(observe: mutation::Observer<'a>) -> Self {
        Self {
            observe,
            prior: None,
            refused: None,
        }
    }

    fn announce(&mut self) -> std::result::Result<(), horizon_cloud::CloudError> {
        match (self.observe)(Mutation::Pending) {
            Ok(prior) => {
                self.prior = Some(prior);
                Ok(())
            }
            Err(error) => {
                self.refused = Some(error);
                Err(horizon_cloud::CloudError::Persistence)
            }
        }
    }
}

fn definite_rejection(error: &horizon_cloud::CloudError) -> bool {
    matches!(
        error,
        horizon_cloud::CloudError::Unauthorized
            | horizon_cloud::CloudError::Rejected(_)
            | horizon_cloud::CloudError::Cancelled
    )
}

/// Saves the stop intent, then asks the provider to stop the worker when `needed`.
/// The intent is rolled back when nothing was sent or the provider definitely
/// refused; a pending stop is settled only after that rollback is saved and when no
/// earlier stop was left uncertain.
pub(in crate::cloud_runtime) fn request_stop(
    store: &Store,
    state: &mut Deployment,
    needed: bool,
    request: impl FnOnce(Announce<'_>) -> std::result::Result<(), horizon_cloud::CloudError>,
    observe: mutation::Observer<'_>,
) -> Result<()> {
    let previous = (state.stop_requested, state.stage);
    state.stop_requested = true;
    state.stage = Stage::Stopping;
    store.save(state)?;
    if !needed {
        return Ok(());
    }
    let mut boundary = Boundary::new(observe);
    let Err(error) = request(&mut || boundary.announce()) else {
        return Ok(());
    };
    if let Some(refused) = boundary.refused {
        return Err(refused);
    }
    match boundary.prior {
        // The provider's checks failed before it sent anything.
        None => {
            (state.stop_requested, state.stage) = previous;
            store.save(state)?;
        }
        Some(prior) if definite_rejection(&error) => {
            (state.stop_requested, state.stage) = previous;
            store.save(state)?;
            if previous.1 != Stage::Stopping && prior == Mutation::Settled {
                observe(Mutation::Settled)?;
            }
        }
        Some(_) => {}
    }
    Err(error.into())
}

/// Asks the provider to start the stopped worker when `needed`, then records the
/// resume. A pending start is settled after the resumed state is saved, or after a
/// definite refusal when no earlier mutation was left uncertain.
pub(in crate::cloud_runtime) fn request_resume(
    store: &Store,
    state: &mut Deployment,
    needed: bool,
    request: impl FnOnce(Announce<'_>) -> std::result::Result<(), horizon_cloud::CloudError>,
    observe: mutation::Observer<'_>,
) -> Result<()> {
    let requested = std::time::SystemTime::now();
    if needed {
        let mut boundary = Boundary::new(observe);
        if let Err(error) = request(&mut || boundary.announce()) {
            if let Some(refused) = boundary.refused {
                return Err(refused);
            }
            if boundary.prior == Some(Mutation::Settled) && definite_rejection(&error) {
                observe(Mutation::Settled)?;
            }
            return Err(error.into());
        }
    }
    state.timeline = Some(super::super::timeline::Timeline::resume_requested(requested));
    state.stop_requested = false;
    state.stage = Stage::Readiness;
    store.save(state)?;
    observe(Mutation::Settled)?;
    Ok(())
}

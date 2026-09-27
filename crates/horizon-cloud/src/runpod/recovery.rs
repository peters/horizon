//! Read-only provider reconciliation. Operator hints are evidence locators, never overrides.
use super::{RunPod, bind};
use crate::{Cancellation, CloudError, CreateState, Worker, WorkerSpec, valid_id};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Prepared,
    Found { worker_id: String },
    Inactive { worker_id: String },
    Unresolved,
    Conflicting { worker_ids: Vec<String> },
    Missing { worker_id: String },
    Terminated { worker_id: String },
}

impl Outcome {
    #[must_use]
    pub const fn needs_attention(&self) -> bool {
        match self {
            Self::Prepared | Self::Found { .. } | Self::Terminated { .. } => false,
            Self::Inactive { .. } | Self::Unresolved | Self::Conflicting { .. } | Self::Missing { .. } => true,
        }
    }

    #[must_use]
    pub const fn explanation(&self) -> &'static str {
        match self {
            Self::Prepared => "No creation request is outstanding. Deployment requires a separate explicit action.",
            Self::Found { .. } => {
                "The provider confirmed the existing worker. Reconnect explicitly to continue on that worker; this check did not start sessions."
            }
            Self::Inactive { .. } => {
                "The provider confirmed this worker's identity but does not report it running. It may be stopped or pending termination; cleanup is not confirmed. Check again or explicitly delete the existing worker. This operation will not allocate a replacement."
            }
            Self::Unresolved => {
                "The provider has not confirmed the creation outcome. An empty listing does not prove failure. Check again, or use a worker ID confirmed by the provider. If no evidence is available, retain this cloud and ask provider support to resolve the original request. Creating a replacement can cause duplicate charges."
            }
            Self::Conflicting { .. } => {
                "Multiple workers match this operation. Ask the provider to resolve the conflicting allocations. No worker was adopted or deleted."
            }
            Self::Missing { .. } => {
                "The previously confirmed worker is no longer returned by the provider. Its identity is retained; missing processes and files are not recreated."
            }
            Self::Terminated { .. } => {
                "Worker termination is confirmed. Its identity is retained; this operation will not create another worker."
            }
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Reconciliation {
    pub operation_id: String,
    pub outcome: Outcome,
    #[serde(skip)]
    pub worker: Option<Worker>,
}

impl RunPod {
    /// Queries provider evidence without creating, starting, stopping or deleting a worker.
    /// A supplied ID is only a lookup hint and must match the immutable operation identity.
    /// The caller must hold its durable operation lock, as for `ensure`.
    /// # Errors
    /// Preserves the fence on cancellation, invalid hints, identity conflicts and provider errors.
    pub fn reconcile(
        &self,
        spec: &WorkerSpec,
        state: &mut CreateState,
        worker_hint: Option<&str>,
        cancel: &Cancellation,
        mut persist: impl FnMut(&CreateState) -> Result<(), CloudError>,
    ) -> Result<Reconciliation, CloudError> {
        spec.validate()?;
        cancel.check()?;
        if worker_hint.is_some_and(|id| !valid_id(id)) {
            return Err(CloudError::Invalid("Invalid worker ID"));
        }
        let mut result = Reconciliation {
            operation_id: spec.operation_id.clone(),
            outcome: Outcome::Unresolved,
            worker: None,
        };
        match state {
            CreateState::Prepared => {
                if worker_hint.is_some() {
                    return Err(CloudError::Invalid("No uncertain creation request to reconcile"));
                }
                result.outcome = Outcome::Prepared;
            }
            CreateState::Terminated { worker_id } => {
                check_hint(worker_hint, worker_id)?;
                result.outcome = Outcome::Terminated {
                    worker_id: worker_id.clone(),
                };
            }
            CreateState::Bound { worker_id } => {
                check_hint(worker_hint, worker_id)?;
                result.worker = self.inspect(worker_id, cancel)?;
                result.outcome = Outcome::Missing {
                    worker_id: worker_id.clone(),
                };
            }
            CreateState::Requested => {
                let mut matches = self
                    .list(cancel)?
                    .into_iter()
                    .filter(|worker| {
                        worker.name == spec.name()
                            || worker.env.get("HORIZON_CLOUD_OPERATION") == Some(&spec.operation_id)
                    })
                    .collect::<Vec<_>>();
                if let Some(id) = worker_hint
                    && !matches.iter().any(|worker| worker.id == id)
                    && let Some(worker) = self.inspect(id, cancel)?
                {
                    worker.verify(spec)?;
                    matches.push(worker);
                }
                if matches.len() > 1 {
                    result.outcome = Outcome::Conflicting {
                        worker_ids: matches.into_iter().map(|worker| worker.id).collect(),
                    };
                    return Ok(result);
                }
                result.worker = matches.pop();
            }
        }
        if let Some(worker) = &result.worker {
            worker.verify(spec)?;
            cancel.check()?;
            if *state == CreateState::Requested {
                bind(state, worker, &mut persist)?;
            }
            result.outcome = if worker.is_starting_or_running() {
                Outcome::Found {
                    worker_id: worker.id.clone(),
                }
            } else {
                Outcome::Inactive {
                    worker_id: worker.id.clone(),
                }
            };
        }
        Ok(result)
    }
}

fn check_hint(hint: Option<&str>, bound: &str) -> Result<(), CloudError> {
    if hint.is_some_and(|id| id != bound) {
        return Err(CloudError::IdentityMismatch);
    }
    Ok(())
}

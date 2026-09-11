use super::{
    RemoteGitObservation, RemoteGitReason as Reason, RemoteGitSetupError as Error, RemoteGitState as State,
    RemoteGitSubmission as Submission,
};
use crate::{
    cloud_run::StoredRemoteAllocation,
    repository_git::{CHECKOUT, GitPreparation, RESPONSE_LIMIT},
};
use serde::{Deserialize, Deserializer};

pub(super) const STATUS_LIMIT: usize = RESPONSE_LIMIT;
pub(super) const LAUNCH_LIMIT: usize = 2 * RESPONSE_LIMIT;

pub(super) fn request(allocation: &StoredRemoteAllocation, branch: &str) -> Result<Vec<u8>, Error> {
    let state = allocation.workspace().state();
    if branch.is_empty() || state.spec.repository.branch.as_deref() != Some(branch) {
        return Err(Error::InvalidBinding);
    }
    let runtime = state.runtime.as_ref().ok_or(Error::InvalidBinding)?;
    let request = GitPreparation {
        version: 1,
        workspace_local_id: state.spec.workspace_local_id.clone(),
        runtime_id: runtime.job_id.to_string().parse().map_err(|_| Error::InvalidBinding)?,
        source: state.spec.repository.clone(),
        work_branch: branch.into(),
    };
    let bytes = serde_json::to_vec(&request).map_err(|_| Error::InvalidBinding)?;
    GitPreparation::decode(&bytes).map_err(|_| Error::InvalidBinding)?;
    Ok(bytes)
}

// A nullable field must still be present; serde's default Option behavior accepts omission.
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Option<T>, D::Error> {
    Option::deserialize(deserializer)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    version: u8,
    state: State,
    #[serde(deserialize_with = "nullable")]
    reason: Option<Reason>,
    #[serde(deserialize_with = "nullable")]
    checkout: Option<String>,
}

impl Observation {
    fn checked(self, code: i32) -> Result<RemoteGitObservation, Error> {
        let expected = match (self.state, self.reason) {
            (State::Absent | State::Complete, None) => 0,
            (_, Some(Reason::Invalid | Reason::Unsupported)) => 2,
            _ => 1,
        };
        if self.version != 1
            || code != expected
            || self.checkout.as_deref() != (self.state == State::Complete).then_some(CHECKOUT)
            || self.state == State::Absent && self.reason.is_some()
            || self.state == State::Error && self.reason.is_none()
        {
            return Err(Error::OutcomeUnknown);
        }
        Ok(RemoteGitObservation {
            state: self.state,
            reason: self.reason,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LaunchState {
    Submitted,
    Observed,
    HandoffUnconfirmed,
    Rejected,
    Error,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Launch {
    version: u8,
    state: LaunchState,
    #[serde(deserialize_with = "nullable")]
    observation: Option<Observation>,
}

pub(super) fn response(bytes: &[u8], code: Option<i32>, observe: bool) -> Result<Submission, Error> {
    let limit = if observe { STATUS_LIMIT } else { LAUNCH_LIMIT };
    if bytes.len() > limit {
        return Err(Error::OutcomeUnknown);
    }
    let code = code.ok_or(Error::OutcomeUnknown)?;
    if observe {
        let observation: Observation = serde_json::from_slice(bytes).map_err(|_| Error::OutcomeUnknown)?;
        return observation.checked(code).map(Submission::Observed);
    }
    let reply: Launch = serde_json::from_slice(bytes).map_err(|_| Error::OutcomeUnknown)?;
    if reply.version != 1 {
        return Err(Error::OutcomeUnknown);
    }
    match (reply.state, reply.observation, code) {
        (LaunchState::Submitted, None, 0) => Ok(Submission::Submitted),
        (LaunchState::HandoffUnconfirmed, None, 1) => Ok(Submission::Unknown),
        (LaunchState::Rejected, None, 2) => Err(Error::Rejected),
        (LaunchState::Error, None, 1) => Err(Error::Unavailable),
        (LaunchState::Observed, Some(observation), code) => observation.checked(code).map(Submission::Observed),
        _ => Err(Error::OutcomeUnknown),
    }
}

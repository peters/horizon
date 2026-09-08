mod request;
mod response;

use horizon_core::repository_overlay::retained_setup::{
    RetainedSetup, SetupAdmission, SetupCompletion, SetupIntent, SetupObservation,
};
use std::{
    io::{Read, Write},
    process::ExitCode,
};

const VERSION: u32 = 1;

#[derive(Clone, Copy)]
pub(super) enum Command {
    Execute,
    Observe,
}

enum Outcome {
    Rejected,
    Error(String),
    Absent,
    ClaimedUnknown,
    Completed {
        receipt: SetupCompletion,
        observed: bool,
    },
    RecordingUnconfirmed {
        reason: String,
        execution: Option<SetupCompletion>,
    },
}

pub(super) fn run(
    command: Command,
    input: &mut impl Read,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
) -> ExitCode {
    let outcome = request::read(input).map_or(Outcome::Rejected, |request| execute(&request, command));
    let response = response::Response::from_outcome(&outcome);
    super::write_response(
        &response,
        response.exit_code(),
        request::RESPONSE_LIMIT,
        output,
        diagnostics,
    )
}

fn execute(request: &request::Request, command: Command) -> Outcome {
    let store = match RetainedSetup::open(&request.retained_root) {
        Ok(store) => store,
        Err(error) => return Outcome::Error(error.to_string()),
    };
    if matches!(command, Command::Observe) {
        return match store.observe(&request.intent) {
            Ok(SetupObservation::Absent) => Outcome::Absent,
            Ok(SetupObservation::ClaimedUnknown) => observe(&store, &request.intent),
            Err(error) => Outcome::Error(error.to_string()),
        };
    }
    match store.admit_materialization(request.intent.clone()) {
        Ok(SetupAdmission::Existing) => observe(&store, &request.intent),
        Ok(SetupAdmission::Fresh(grant)) => match grant.materialize_recorded(|| false) {
            Ok(execution) => Outcome::Completed {
                receipt: SetupCompletion::from_execution(&execution),
                observed: false,
            },
            Err(error) => Outcome::RecordingUnconfirmed {
                reason: error.to_string(),
                execution: error.execution().map(SetupCompletion::from_execution),
            },
        },
        Err(error) => Outcome::Error(error.to_string()),
    }
}

fn observe(store: &RetainedSetup, intent: &SetupIntent) -> Outcome {
    match store.completion(intent) {
        Ok(Some(receipt)) => Outcome::Completed {
            receipt,
            observed: true,
        },
        Ok(None) => Outcome::ClaimedUnknown,
        Err(error) => Outcome::Error(error.to_string()),
    }
}

#[cfg(test)]
mod tests;

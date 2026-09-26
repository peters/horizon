use super::{Change, Error, Intent, Journal, Result, Status, Step};
use crate::cloud_runtime::project_reservations::Reservation;
use horizon_cloud_protocol::{
    bootstrap::RecoveryRequest,
    membership::{Request, Source},
    signed::SignedIntent,
};
use std::collections::BTreeSet;

fn step(intent: &Intent, index: usize) -> Step {
    match index {
        0 => Step::Reserve,
        1 => Step::PrepareNamespace,
        2 => Step::ImportSource,
        _ => {
            let offset = index - 3;
            if offset < intent.sessions.len() * 2 {
                let session = intent.sessions[offset / 2].id;
                if offset.is_multiple_of(2) {
                    Step::ReserveSession(session)
                } else {
                    Step::PrepareSession(session)
                }
            } else {
                intent
                    .sessions
                    .get(offset - intent.sessions.len() * 2)
                    .map_or(Step::Complete, |s| Step::StartSession(s.id))
            }
        }
    }
}
fn descriptor(intent: &Intent, journal: Option<&Journal>) -> Result<Source> {
    journal
        .and_then(|j| {
            j.sources
                .iter()
                .find(|s| s.matches(&intent.request.project, &intent.request.repository, &intent.revision))
        })
        .map(|s| s.descriptor.clone())
        .ok_or(Error::Invalid)
}
fn payload(intent: &Intent, next: &Step, journal: Option<&Journal>) -> Result<Request> {
    Ok(match next {
        Step::Reserve => Request::Reserve {
            capabilities: intent.request.capabilities.clone(),
            ports: BTreeSet::new(),
        },
        Step::PrepareNamespace => Request::PrepareNamespace {},
        Step::ImportSource => Request::ImportSource {
            descriptor: descriptor(intent, journal)?,
        },
        Step::ReserveSession(id) => Request::ReserveSession {
            session: intent
                .sessions
                .iter()
                .find(|s| s.id == *id)
                .ok_or(Error::Invalid)?
                .clone(),
        },
        Step::PrepareSession(id) => Request::PrepareSession { session_id: *id },
        Step::StartSession(id) => Request::StartSession { session_id: *id },
        Step::Complete | Step::Terminal => return Err(Error::Invalid),
    })
}
pub(super) fn inspect(intent: &Intent, journal: Option<&Journal>) -> Result<Status> {
    let mut index = 0;
    let mut terminal = false;
    let mut pending = false;
    if let Some(journal) = journal {
        if journal.binding != intent.binding || journal.image_digest != intent.request.image_digest {
            return Err(Error::Invalid);
        }
        if journal.generation.is_some() {
            return Err(Error::Blocked);
        }
        for operation in &journal.manifest.operations {
            if operation.receipt.identity.project_id() != intent.request.project.project_id() {
                continue;
            }
            if operation.receipt.identity != intent.request.project {
                return Err(Error::Invalid);
            }
            let request: Request = serde_json::from_str(&operation.payload).map_err(|_| Error::Invalid)?;
            if matches!(request, Request::StopSession { .. } | Request::Cancel {}) {
                terminal = true;
            } else if terminal || request != payload(intent, &step(intent, index), Some(journal))? {
                return Err(Error::Invalid);
            } else {
                index += 1;
            }
        }
        if let Some(retained) = &journal.pending {
            // A terminal or unrelated pending operation is never completed by setup.
            let request: RecoveryRequest = serde_json::from_str(&retained.request).map_err(|_| Error::Invalid)?;
            let mutation: Request = serde_json::from_str(&request.payload).map_err(|_| Error::Invalid)?;
            if terminal
                || retained.receipt.identity != intent.request.project
                || mutation != payload(intent, &step(intent, index), Some(journal))?
            {
                return Err(Error::Blocked);
            }
            let signed = SignedIntent::parse(request.message.as_bytes()).map_err(|_| Error::Invalid)?;
            let _ = signed
                .verify(&intent.binding.startup.controller, request.payload.as_bytes())
                .map_err(|_| Error::Invalid)?;
            pending = true;
        }
    }
    Ok(Status {
        project: intent.request.project.clone(),
        revision: intent.revision.clone(),
        sessions: intent.sessions.clone(),
        next: if terminal { Step::Terminal } else { step(intent, index) },
        pending,
    })
}
pub(super) fn change(intent: &Intent, next: &Step, journal: Option<&Journal>) -> Result<Change> {
    let project = intent.request.project.clone();
    Ok(match payload(intent, next, journal)? {
        Request::Reserve { capabilities, ports } => Change::Reserve(Reservation {
            project,
            image_digest: intent.request.image_digest.clone(),
            capabilities,
            ports,
        }),
        Request::PrepareNamespace {} => Change::PrepareNamespace(project),
        Request::ImportSource { descriptor } => Change::ImportSource(project, descriptor),
        Request::ReserveSession { session } => Change::ReserveSession(project, session),
        Request::PrepareSession { session_id } => Change::PrepareSession(project, session_id),
        Request::StartSession { session_id } => Change::StartSession(project, session_id),
        _ => return Err(Error::Invalid),
    })
}

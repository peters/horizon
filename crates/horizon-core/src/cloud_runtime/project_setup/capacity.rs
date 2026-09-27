//! Conservative admission against the protocol's retained history and cleanup bounds.
use super::{Error, Owner, Result, Step, intent::Registry, progress};
use crate::cloud_runtime::project_reservations::journal::Journal;
use horizon_cloud_protocol::{
    OperationId,
    membership::{Artifact, CANCELLATION_BYTES, MAX_MANIFEST_BYTES, Manifest, Request, Source, State},
    signed::{Intent, Target},
};

pub(super) fn require(registry: &Registry, owner: &Owner, journal: Option<&Journal>) -> Result<()> {
    let first = registry.intents.first().ok_or(Error::Invalid)?;
    let mut projected = journal.map_or_else(
        || Manifest::empty(first.binding.startup.clone(), first.binding.worker_id.clone()),
        |saved| saved.manifest.clone(),
    );
    let mut hypothetical = 0;
    for saved in &registry.intents {
        let next = progress::inspect(saved, journal)?.next;
        if matches!(next, Step::Complete | Step::Terminal) {
            continue;
        }
        for step in progress::steps(saved).skip_while(|step| *step != next) {
            let request = if step == Step::ImportSource {
                Request::ImportSource {
                    descriptor: maximum_source(&saved.revision),
                }
            } else {
                progress::payload(saved, &step, journal)?
            };
            let payload = serde_json::to_string(&request).map_err(|_| Error::Invalid)?;
            let intent = Intent::new(
                &first.binding.startup.controller,
                OperationId::generate(),
                projected.revision,
                Target::Project {
                    identity: saved.request.project.clone(),
                },
                request.action(),
                payload.as_bytes(),
            )
            .map_err(|_| Error::Invalid)?;
            let message = serde_json::to_string(&owner.sign(intent)?).map_err(|_| Error::Invalid)?;
            projected = projected.next(&message, &payload).map_err(|_| Error::Capacity)?.0;
            hypothetical += 1;
        }
    }
    let cleanup: usize = projected
        .members
        .iter()
        .filter(|member| member.state != State::Removed)
        .map(|member| 1 + member.launches.iter().filter(|id| !member.stops.contains(id)).count())
        .sum();
    // Future signatures (64 bytes) and payload/receipt hashes (32 each) can
    // gain two decimal digits per byte. Interleaving can grow two revision
    // fields by one digit each; the manifest's own revision gets one byte too.
    let uncertainty = hypothetical * (2 * (64 + 32 + 32) + 2) + 1;
    if serde_json::to_vec(&projected).map_err(|_| Error::Invalid)?.len() + uncertainty + cleanup * CANCELLATION_BYTES
        > MAX_MANIFEST_BYTES
    {
        return Err(Error::Capacity);
    }
    Ok(())
}

fn maximum_source(revision: &str) -> Source {
    // Both lengths have the maximum valid decimal width, and all hash bytes
    // have the maximum encoded width. No source export or worker send occurs.
    Source {
        version: 1,
        revision: revision.into(),
        pack: Artifact {
            length: Source::MAX_BYTES / 2,
            sha256: [255; 32],
        },
        material: Artifact {
            length: Source::MAX_BYTES / 2,
            sha256: [255; 32],
        },
    }
}

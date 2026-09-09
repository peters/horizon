use super::{
    request::Request,
    response::{Response, Status},
};
use horizon_core::repository_overlay::seed::receive::{
    ExpectedGitPack, PackReceiveLimits, observe_git_base_pack,
    publication::{PackPublicationFailure, publish_sibling_git_pack},
    receive_git_base_pack,
};
use std::io::Read;

pub(super) fn execute(request: &Request, mut input: &mut dyn Read) -> Response {
    let Ok(base_commit) = request.pack.base_commit.as_str().parse() else {
        return Response::new(Status::Rejected, "invalid pack identity");
    };
    let expected = ExpectedGitPack {
        base_commit,
        sha256: &request.pack.sha256,
        encoded_bytes: request.pack.encoded_bytes,
    };
    let limits = PackReceiveLimits::default();
    let Some(destination) = &request.destination else {
        return match observe_git_base_pack(&request.path, expected, limits, || false) {
            Ok(pack) => Response::verified(Status::Observed, &pack, &request.pack),
            Err(_) => Response::new(
                Status::Error,
                "pack could not be safely observed; retain data and do not replay",
            ),
        };
    };
    let pack = match receive_git_base_pack(&request.path, expected, &mut input, limits, || false) {
        Ok(pack) => pack,
        Err(failure) => {
            return match failure.residue() {
                Some(path) => Response::retained(
                    Status::ReceiveUnconfirmed,
                    Some(path.to_owned()),
                    None,
                    "pack receipt failed; retain unconfirmed input and do not replay",
                ),
                None => Response::new(Status::Error, "pack receipt failed before a reservation was returned"),
            };
        }
    };
    match publish_sibling_git_pack(pack, destination, limits, || false) {
        Ok(pack) => Response::verified(Status::Acknowledged, pack.pack(), &request.pack),
        Err(failure) => publication_failure(failure),
    }
}

fn publication_failure(failure: PackPublicationFailure) -> Response {
    match failure {
        PackPublicationFailure::Unpublished { pack, .. } => Response::retained(
            Status::Unpublished,
            Some(pack.path().to_owned()),
            None,
            "pack was not published; retain input and do not replay",
        ),
        PackPublicationFailure::PublishedUnsynchronized { pack, .. } => Response::retained(
            Status::PublishedUnsynchronized,
            None,
            Some(pack.path().to_owned()),
            "pack was renamed without final confirmation; retain destination and observe",
        ),
        PackPublicationFailure::RenameUnconfirmed { pack, destination } => Response::retained(
            Status::RenameUnconfirmed,
            Some(pack.path().to_owned()),
            Some(destination),
            "pack rename is unconfirmed; retain and inspect both candidate names",
        ),
    }
}

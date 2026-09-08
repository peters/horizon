use horizon_core::{
    cloud_run::ArtifactDigest,
    repository_overlay::{
        checkout::publication::PublicationFailure,
        materialize::{
            MAX_REQUEST_PATH_BYTES, MaterializationFailure, MaterializationProblem, MaterializationRequest,
            MaterializedRepository, materialize_repository,
        },
    },
};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

const REQUEST_LIMIT: usize = 16 * 1024;
// An uncertain rename reports three paths: allow six-byte JSON escaping, one
// bounded child component per path, fixed metadata and the final newline.
pub(super) const RESPONSE_LIMIT: usize = (3 * 6 * (MAX_REQUEST_PATH_BYTES + 256) + 4096 + 1).next_power_of_two();
const VERSION: u32 = 1;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    version: u32,
    objects_directory: PathBuf,
    bundle_store: PathBuf,
    bundle_manifest: ArtifactDigest,
    scratch_parent: PathBuf,
    destination: String,
}

pub(super) fn read_request(input: &mut impl Read) -> Result<Request, ()> {
    let mut bytes = Vec::new();
    input
        .take(REQUEST_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() > REQUEST_LIMIT {
        return Err(());
    }
    let request: Request = serde_json::from_slice(&bytes).map_err(|_| ())?;
    if request.version != VERSION {
        return Err(());
    }
    Ok(request)
}

pub(super) fn execute(request: &Request) -> Result<MaterializedRepository, MaterializationFailure> {
    materialize_repository(
        &MaterializationRequest {
            objects_directory: &request.objects_directory,
            bundle_store: &request.bundle_store,
            bundle_manifest: &request.bundle_manifest,
            scratch_parent: &request.scratch_parent,
            destination: &request.destination,
        },
        || false,
    )
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Rejected,
    Unpublished,
    Published,
    PublishedUnsynchronized,
    RenameUnconfirmed,
}

#[derive(Serialize)]
pub(super) struct Response<'a> {
    version: u32,
    status: Status,
    reason: Option<String>,
    source_metadata: Option<&'a Path>,
    checkout: Option<&'a Path>,
    possible_destination: Option<&'a Path>,
    base_commit: Option<String>,
    bundle_manifest: Option<&'a ArtifactDigest>,
}

impl<'a> Response<'a> {
    pub(super) fn rejected() -> Self {
        Self {
            version: VERSION,
            status: Status::Rejected,
            reason: Some("invalid or unsupported materialization request".into()),
            source_metadata: None,
            checkout: None,
            possible_destination: None,
            base_commit: None,
            bundle_manifest: None,
        }
    }

    pub(super) fn exit_code(&self) -> u8 {
        match self.status {
            Status::Published => 0,
            Status::Rejected => 2,
            Status::Unpublished | Status::PublishedUnsynchronized | Status::RenameUnconfirmed => 1,
        }
    }

    pub(super) fn from_result(result: &'a Result<MaterializedRepository, MaterializationFailure>) -> Self {
        let mut response = Self::rejected();
        match result {
            Ok(repository) => {
                response.status = Status::Published;
                response.reason = None;
                response.source_metadata = Some(repository.source_metadata());
                response.checkout = Some(repository.checkout().path());
                response.base_commit = Some(repository.checkout().base_commit().to_string());
                response.bundle_manifest = Some(repository.checkout().manifest_sha256());
            }
            Err(failure) => {
                response.status = Status::Unpublished;
                response.reason = Some(failure.to_string());
                response.source_metadata = failure.source_metadata();
                match &failure.problem {
                    MaterializationProblem::InvalidRequest => response.status = Status::Rejected,
                    MaterializationProblem::Preparation(failure) => response.checkout = failure.residue(),
                    MaterializationProblem::Publication(failure) => response.publication(failure),
                    _ => {}
                }
            }
        }
        response
    }

    fn publication(&mut self, failure: &'a PublicationFailure) {
        match failure {
            PublicationFailure::Unpublished { checkout, .. } => {
                self.checkout = Some(checkout.path());
                self.base_commit = Some(checkout.base_commit().to_string());
                self.bundle_manifest = Some(checkout.manifest_sha256());
            }
            PublicationFailure::PublishedUnsynchronized { checkout, .. } => {
                self.status = Status::PublishedUnsynchronized;
                self.checkout = Some(checkout.path());
                self.base_commit = Some(checkout.base_commit().to_string());
                self.bundle_manifest = Some(checkout.manifest_sha256());
            }
            PublicationFailure::RenameUnconfirmed { checkout, destination } => {
                self.status = Status::RenameUnconfirmed;
                self.checkout = Some(checkout.path());
                self.possible_destination = Some(destination);
                self.base_commit = Some(checkout.base_commit().to_string());
                self.bundle_manifest = Some(checkout.manifest_sha256());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io;

    fn request() -> serde_json::Value {
        json!({"version":1,"objects_directory":"/synthetic/objects",
            "bundle_store":"/synthetic/store","bundle_manifest":"a".repeat(64),
            "scratch_parent":"/synthetic/private","destination":"ready"})
    }

    fn accepts(value: &serde_json::Value) -> bool {
        read_request(&mut serde_json::to_vec(value).unwrap().as_slice()).is_ok()
    }

    #[test]
    fn version_digest_fields_and_framing_are_strict_and_bounded() {
        let original = request();
        assert!(accepts(&original));
        for key in original.as_object().unwrap().keys() {
            let mut value = original.clone();
            value.as_object_mut().unwrap().remove(key);
            assert!(!accepts(&value));
        }
        for (key, value) in [
            ("version", json!(2)),
            ("version", json!(-1)),
            ("version", json!(1.5)),
            ("bundle_manifest", json!("secret-invalid-digest")),
            ("bundle_manifest", json!("g".repeat(64))),
            ("unknown", json!(true)),
            ("destination", json!(null)),
        ] {
            let mut altered = original.clone();
            altered[key] = value;
            assert!(!accepts(&altered));
        }
        let encoded = serde_json::to_string(&original).unwrap();
        for malformed in [
            format!("{encoded}{{}}"),
            encoded.replacen('{', "{\"version\":1,", 1),
            String::new(),
        ] {
            assert!(read_request(&mut malformed.as_bytes()).is_err());
        }
        let mut exact = encoded.into_bytes();
        exact.resize(REQUEST_LIMIT, b' ');
        assert!(read_request(&mut exact.as_slice()).is_ok());
        exact.push(b' ');
        assert!(read_request(&mut exact.as_slice()).is_err());
        let mut endless = io::repeat(b' ');
        assert!(read_request(&mut endless).is_err());
        // A finite wrapper independently measures bytes consumed from an oversized stream.
        let bytes = vec![b' '; REQUEST_LIMIT * 2];
        let mut cursor = io::Cursor::new(bytes);
        assert!(read_request(&mut cursor).is_err());
        assert_eq!(cursor.position(), REQUEST_LIMIT as u64 + 1);
    }

    #[test]
    fn escaped_retained_paths_fit_the_complete_response_bound() {
        let components: [String; 11] = std::array::from_fn(|_| "\u{1}".repeat(230));
        let scratch = PathBuf::from(format!("/{}", components.join("/")));
        let mut value = request();
        value["scratch_parent"] = json!(scratch);
        let encoded = serde_json::to_vec(&value).unwrap();
        assert!(encoded.len() < REQUEST_LIMIT);
        assert!(read_request(&mut encoded.as_slice()).is_ok());
        // Pure receipt-format regression, not a claim that a rename was performed.
        for parent in [
            scratch,
            PathBuf::from(format!("/{}", "\u{1}".repeat(MAX_REQUEST_PATH_BYTES - 1))),
        ] {
            let metadata = parent.join("m".repeat(255));
            let checkout = parent.join("c".repeat(255));
            let destination = parent.join("d".repeat(255));
            let mut response = Response::rejected();
            response.status = Status::RenameUnconfirmed;
            response.source_metadata = Some(&metadata);
            response.checkout = Some(&checkout);
            response.possible_destination = Some(&destination);
            response.base_commit = Some("a".repeat(40));
            let digest = ArtifactDigest::sha256(b"synthetic");
            response.bundle_manifest = Some(&digest);
            let bytes = serde_json::to_vec(&response).unwrap();
            assert!(bytes.len() > 32 * 1024);
            assert!(bytes.len() < RESPONSE_LIMIT, "complete JSON plus newline must fit");
        }
    }

    #[test]
    fn relative_paths_and_unsafe_names_fail_before_io_and_response_is_redacted() {
        let mut value = request();
        value["objects_directory"] = json!("private-relative-marker");
        let parsed = read_request(&mut serde_json::to_vec(&value).unwrap().as_slice()).unwrap();
        let result = execute(&parsed);
        let response = Response::from_result(&result);
        assert_eq!(response.exit_code(), 2);
        let bytes = serde_json::to_vec(&response).unwrap();
        assert!(
            !String::from_utf8(bytes.clone())
                .unwrap()
                .contains("private-relative-marker")
        );
        let response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(response["status"], "rejected");
        assert!(response["source_metadata"].is_null() && response["checkout"].is_null());
    }
}

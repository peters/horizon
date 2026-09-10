//! Synthetic exchange/pack bytes isolate controller fences, not native Git decoding
//! or the public `PreparedGitPack` constructor; the pinned smoke must exercise those.
use super::*;
use crate::{
    cloud_run::interactive_worker::InteractiveWorkerLifecycle, remote_repository_pack::tests::Fixture,
    repository_overlay::RepositoryOverlayPlan,
};
use ControllerError as Error;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::{
        fs::{PermissionsExt, symlink},
        process::ExitStatusExt,
    },
    path::PathBuf,
    process::ExitStatus,
};

const PACKS: &str = "/workspace/.horizon-worker/repository-inputs/packs";
const PACK: [u8; 64] = [b'x'; 64];

struct InputFixture {
    control: Fixture,
    path: PathBuf,
    overlay: Vec<u8>,
    request: IntakeRequest,
}

impl InputFixture {
    fn new() -> Self {
        let control = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
        let allocation = control.recovered.allocation();
        let path = control.directory.path().join("synthetic.pack");
        fs::write(&path, PACK).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let source = allocation.workspace().state().spec.repository.clone();
        let plan = RepositoryOverlayPlan::new(source, [], []).unwrap();
        let bundle = RepositoryOverlayBundle::new(plan, []).unwrap();
        let overlay = codec::encode(&bundle).unwrap().into_vec();
        let request = bound_request(
            allocation,
            EncodedIdentity {
                sha256: ArtifactDigest::sha256(&PACK),
                encoded_bytes: PACK.len() as u64,
            },
            EncodedIdentity {
                sha256: bundle.manifest_sha256().clone(),
                encoded_bytes: overlay.len() as u64,
            },
        )
        .unwrap();
        Self {
            control,
            path,
            overlay,
            request,
        }
    }

    fn check(
        &self,
        send: bool,
        cancelled: &dyn Fn() -> bool,
        execute: impl FnOnce(&mut dyn Read, &dyn Fn() -> bool) -> Result<query::Exchange, Error>,
    ) -> Result<IntakeResponse, Box<ControllerFailure>> {
        let proposal = RepositoryIntakeProposal {
            allocation: self.control.recovered.allocation(),
            path: &self.path,
            request: self.request.clone(),
            overlay: self.overlay.clone(),
        };
        perform(
            &self.control.store,
            &self.control.recovered,
            self.request.clone(),
            send.then(|| proposal.approve_export().0).as_ref(),
            cancelled,
            |_, observe, input, cancelled| {
                assert_eq!(observe, !send);
                execute(input, cancelled)
            },
        )
    }

    fn response(&self, state: &str) -> Value {
        json!({"version":1,"state":state,"intent_sha256":ArtifactDigest::sha256(&self.request.encode().unwrap()),
            "roots":{"packs":"/workspace/.horizon-worker/repository-inputs/packs","bundles":"/workspace/.horizon-worker/repository-inputs/bundles","setup":"/workspace/.horizon-worker/repository-setup"},
            "pack":{"state":state,"source":null,"destination":"/workspace/.horizon-worker/repository-inputs/packs/base","objects":2},
            "bundle":state,"reason":null})
    }
}

fn wire(value: &Value, complete: bool) -> query::Exchange {
    let code = match value["state"].as_str().unwrap() {
        "acknowledged" | "observed" => 0,
        "rejected" => 2,
        "claimed_unknown" => 4,
        _ => 1,
    };
    query::Exchange {
        status: ExitStatus::from_raw(code << 8),
        output: serde_json::to_vec(value).unwrap(),
        input: if complete {
            query::InputProgress::Complete(0)
        } else {
            query::InputProgress::Incomplete(0)
        },
    }
}

#[test]
fn synthetic_approval_preserves_exact_frames_ownership_and_early_replies() {
    let fixture = InputFixture::new();
    let allocation = fixture.control.current();
    assert!(allocation.workspace().state().spec.panels.is_empty());
    let key_path = fixture.control.recovered.identity().private_key_path();
    let key = ArtifactDigest::sha256(&fs::read(key_path).unwrap());
    let response = fixture.response("acknowledged");
    let result = fixture
        .check(true, &|| false, |input, _| {
            let mut actual = Vec::new();
            input.read_to_end(&mut actual).unwrap();
            let header = fixture.request.encode().unwrap();
            let mut expected = u32::try_from(header.len()).unwrap().to_le_bytes().to_vec();
            expected.extend(header);
            expected.extend(PACK);
            expected.extend(&fixture.overlay);
            assert_eq!(actual, expected);
            Ok(wire(&response, true))
        })
        .unwrap();
    assert_eq!(result.state, IntakeState::Acknowledged);
    assert_eq!(fixture.control.current(), allocation);
    assert_eq!(ArtifactDigest::sha256(&fs::read(key_path).unwrap()), key);
    assert_eq!(fs::read(&fixture.path).unwrap(), PACK);
    let observed = fixture.response("observed");
    let response = fixture
        .check(true, &|| false, |_, _| Ok(wire(&observed, false)))
        .unwrap();
    assert_eq!(response.state, IntakeState::Observed);
    let ack = fixture.response("acknowledged");
    let failure = fixture
        .check(true, &|| false, |_, _| Ok(wire(&ack, false)))
        .unwrap_err();
    assert!(matches!(failure.reason, Error::Response) && failure.response.is_none());
    fs::remove_file(&fixture.path).unwrap();
    let response = fixture
        .check(false, &|| false, |input, _| {
            let mut bytes = Vec::new();
            input.read_to_end(&mut bytes).unwrap();
            assert_eq!(IntakeRequest::decode(&bytes).unwrap(), fixture.request);
            Ok(wire(&observed, false))
        })
        .unwrap();
    assert_eq!(response.state, IntakeState::Observed);
    let failure = fixture
        .check(false, &|| false, |_, _| Err(Error::Transport))
        .unwrap_err();
    assert_eq!(failure.request, fixture.request);
    assert!(matches!(failure.reason, Error::Transport) && failure.response.is_none());
    assert!(!format!("{failure:?}").contains("synthetic-worker"));
}

#[test]
fn stale_ownership_missing_database_and_unsafe_inputs_never_exchange() {
    for case in 0..9 {
        let mut fixture = InputFixture::new();
        let parent = fixture.control.store.path().parent().unwrap().to_path_buf();
        match case {
            0 | 1 => fixture.control.edit(case == 1),
            2 => fixture.request.worker_resource_id.push_str("-changed"),
            3 => fixture.request.source.repository.push_str("-changed"),
            4 => fs::rename(&parent, parent.with_extension("retained")).unwrap(),
            5 => fs::write(&fixture.path, vec![b'z'; PACK.len()]).unwrap(),
            6 => fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o644)).unwrap(),
            7 => {
                fs::rename(&fixture.path, fixture.path.with_extension("held")).unwrap();
                symlink(fixture.path.with_extension("held"), &fixture.path).unwrap();
            }
            _ => fs::hard_link(&fixture.path, fixture.path.with_extension("linked")).unwrap(),
        }
        let failure = fixture
            .check(true, &|| false, |_, _| panic!("no exchange"))
            .unwrap_err();
        assert!(matches!(
            (case, failure.reason),
            (0 | 1 | 4, Error::Admission(_)) | (2 | 3, Error::Request) | (5..=8, Error::Input)
        ));
        assert!(failure.response.is_none());
        assert_eq!(failure.request, fixture.request);
        assert!(fixture.path.exists());
        if case == 4 {
            assert!(!parent.exists());
        }
    }
}

#[test]
fn late_ownership_input_or_cancellation_changes_preserve_receipts() {
    for case in 0..3 {
        let fixture = InputFixture::new();
        let cancelled = std::cell::Cell::new(false);
        let value = fixture.response("observed");
        let failure = fixture
            .check(true, &|| cancelled.get(), |_, _| {
                match case {
                    0 => fixture.control.edit(false),
                    1 => fs::write(&fixture.path, b"changed").unwrap(),
                    _ => cancelled.set(true),
                }
                Ok(wire(&value, false))
            })
            .unwrap_err();
        assert!(matches!(
            (case, failure.reason),
            (0, Error::Admission(_)) | (1, Error::Input) | (2, Error::Cancelled)
        ));
        assert_eq!(serde_json::to_value(failure.response.unwrap()).unwrap(), value);
        assert_eq!(failure.request, fixture.request);
        assert!(fixture.path.exists());
    }
}

#[test]
fn one_shot_exchange_cancellation_preserves_receipts_and_transport_failures() {
    for send in [false, true] {
        for (cancel, reply) in [(false, false), (true, false), (true, true)] {
            let fixture = InputFixture::new();
            let cancel_next = Cell::new(false);
            let value = fixture.response("observed");
            let failure = fixture
                .check(send, &|| cancel_next.replace(false), |_, cancelled| {
                    cancel_next.set(cancel);
                    assert_eq!(cancelled(), cancel);
                    assert!(!cancelled());
                    if reply {
                        Ok(wire(&value, false))
                    } else {
                        Err(Error::Transport)
                    }
                })
                .unwrap_err();
            assert!(matches!(
                (cancel, failure.reason),
                (true, Error::Cancelled) | (false, Error::Transport)
            ));
            assert_eq!(failure.request, fixture.request);
            assert_eq!(
                failure.response.map(|response| serde_json::to_value(response).unwrap()),
                reply.then_some(value)
            );
        }
    }
}

#[test]
fn query_failures_preserve_protocol_and_admission_categories() {
    for send in [false, true] {
        for cancel in [false, true] {
            for (error, expected) in [
                (query::Error::ClientUnavailable, "client-unavailable"),
                (query::Error::OutputLimit, "response"),
                (query::Error::Deadline, "transport"),
                (query::Error::QueryFailed, "transport"),
            ] {
                let fixture = InputFixture::new();
                let cancel_next = Cell::new(false);
                let failure = fixture
                    .check(send, &|| cancel_next.replace(false), |_, cancelled| {
                        cancel_next.set(cancel);
                        assert_eq!(cancelled(), cancel);
                        assert!(!cancelled());
                        Err(query_error(&error))
                    })
                    .unwrap_err();
                assert_eq!(failure.request, fixture.request);
                assert!(failure.response.is_none());
                let actual = match failure.reason {
                    Error::Admission(RemotePanelStatusError::ClientUnavailable) => "client-unavailable",
                    Error::Response => "response",
                    Error::Transport => "transport",
                    Error::Cancelled => "cancelled",
                    other => panic!("unexpected query failure: {other}"),
                };
                assert_eq!(actual, if cancel { "cancelled" } else { expected });
            }
        }
    }
}

#[test]
fn every_retained_publication_projection_and_unknown_progress_roundtrips() {
    let fixture = InputFixture::new();
    for (state, pack, bundle) in [
        ("error", None, None),
        ("unconfirmed", Some("receive_unconfirmed"), None),
        ("unconfirmed", Some("unpublished"), None),
        ("unconfirmed", Some("published_unsynchronized"), None),
        ("unconfirmed", Some("rename_unconfirmed"), None),
        ("unconfirmed", Some("acknowledged"), None),
        ("unconfirmed", Some("acknowledged"), Some("write_unconfirmed")),
        ("unconfirmed", Some("acknowledged"), Some("acknowledged")),
        ("claimed_unknown", None, None),
        ("claimed_unknown", Some("observed"), None),
        ("claimed_unknown", Some("observed"), Some("observed")),
        ("unsupported", None, None),
        ("unsupported", Some("observed"), None),
        ("unsupported", Some("observed"), Some("observed")),
    ] {
        let mut value = fixture.response(state);
        value["reason"] = json!(match state {
            "unsupported" => "unsupported",
            "claimed_unknown" => "input",
            _ => "storage",
        });
        value["bundle"] = json!(bundle);
        value["pack"] = pack.map_or(Value::Null, |pack| {
            json!({
                "state": pack,
                "source": matches!(pack, "receive_unconfirmed" | "unpublished" | "rename_unconfirmed")
                    .then(|| format!("{PACKS}/repository-seed-abc123")),
                "destination": (!matches!(pack, "receive_unconfirmed" | "unpublished"))
                    .then(|| format!("{PACKS}/base")),
                "objects": (pack != "receive_unconfirmed").then_some(2)
            })
        });
        let response = decode(&wire(&value, false), &fixture.request, state != "unconfirmed").unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(), value);
    }
    let portable = serde_json::to_value(IntakeResponse::failure(IntakeError::Unsupported)).unwrap();
    assert!(decode(&wire(&portable, false), &fixture.request, true).is_ok());
}

#[test]
fn malformed_or_inconsistent_replies_are_not_validated_by_a_false_green_control() {
    let fixture = InputFixture::new();
    let valid = fixture.response("acknowledged");
    assert!(decode(&wire(&valid, true), &fixture.request, false).is_ok());
    for (path, value) in [
        ("/version", json!(2)),
        ("/intent_sha256", json!("a".repeat(64))),
        ("/roots", Value::Null),
        ("/roots/packs", json!(format!("{PACKS}//"))),
        ("/pack/destination", json!(format!("{PACKS}/base/."))),
        ("/pack/objects", json!(0)),
        ("/pack/source", json!("/outside")),
        ("/pack", Value::Null),
        ("/bundle", Value::Null),
        ("/reason", json!("storage")),
    ] {
        let mut altered = valid.clone();
        *altered.pointer_mut(path).unwrap() = value;
        assert!(
            decode(&wire(&altered, true), &fixture.request, false).is_err(),
            "{path}"
        );
    }
    for field in ["intent_sha256", "roots", "pack", "bundle", "reason"] {
        let mut altered = valid.clone();
        altered.as_object_mut().unwrap().remove(field);
        assert!(decode(&wire(&altered, true), &fixture.request, false).is_err());
    }
    let mut extra = valid.clone();
    extra["unknown"] = json!(1);
    let mut duplicate = wire(&valid, true);
    duplicate.output.splice(1..1, b"\"version\":1,".iter().copied());
    let mut trailing = wire(&valid, true);
    trailing.output.push(b'x');
    let mut oversized = wire(&valid, true);
    oversized.output = vec![b' '; RESPONSE_LIMIT + 1];
    let mut exit = wire(&valid, true);
    exit.status = ExitStatus::from_raw(1 << 8);
    let unbound = serde_json::to_value(IntakeResponse::failure(IntakeError::Storage)).unwrap();
    for invalid in [
        wire(&extra, true),
        wire(&unbound, true),
        duplicate,
        trailing,
        oversized,
        exit,
    ] {
        assert!(decode(&invalid, &fixture.request, false).is_err());
    }
    assert!(decode(&wire(&valid, true), &fixture.request, true).is_err());
}

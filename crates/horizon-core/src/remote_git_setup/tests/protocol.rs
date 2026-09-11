use super::super::{protocol::*, *};
use serde_json::{Value, json};

fn observation(state: &str, reason: Option<&str>) -> Value {
    json!({"version": 1, "state": state, "reason": reason,
        "checkout": if state == "complete" { Some("/workspace/horizon/repository") } else { None }})
}

fn decode(value: &Value, code: i32, observe: bool) -> Result<RemoteGitSubmission, RemoteGitSetupError> {
    response(&serde_json::to_vec(value).unwrap(), Some(code), observe)
}

#[test]
fn all_observation_exit_pairs_and_degraded_completion_are_strict() {
    for state in ["absent", "claimed_unknown", "complete", "error"] {
        for reason in [
            None,
            Some("invalid"),
            Some("unsupported"),
            Some("unsafe_root"),
            Some("conflict"),
            Some("storage"),
            Some("git"),
            Some("interrupted"),
            Some("unsupported_repository"),
        ] {
            let valid = !(state == "absent" && reason.is_some() || state == "error" && reason.is_none());
            let expected = if matches!(state, "absent" | "complete") && reason.is_none() {
                0
            } else if matches!(reason, Some("invalid" | "unsupported")) {
                2
            } else {
                1
            };
            let value = observation(state, reason);
            for code in [0, 1, 2, 3, 255] {
                assert_eq!(
                    decode(&value, code, true).is_ok(),
                    valid && code == expected,
                    "{state} {reason:?} {code}"
                );
                let wrapped = json!({"version": 1, "state": "observed", "observation": value});
                assert_eq!(
                    decode(&wrapped, code, false).is_ok(),
                    state != "absent" && valid && code == expected,
                    "wrapped {state} {reason:?} {code}"
                );
            }
        }
    }
    assert_eq!(
        decode(&observation("complete", Some("unsafe_root")), 1, true),
        Ok(RemoteGitSubmission::Observed(RemoteGitObservation {
            state: RemoteGitState::Complete,
            reason: Some(RemoteGitReason::UnsafeRoot),
        }))
    );
}

#[test]
fn launch_acknowledgement_never_claims_completion_or_grants_retry() {
    for (state, code, expected) in [
        ("submitted", 0, Ok(RemoteGitSubmission::Submitted)),
        ("handoff_unconfirmed", 1, Ok(RemoteGitSubmission::Unknown)),
        ("rejected", 2, Err(RemoteGitSetupError::Rejected)),
        ("error", 1, Err(RemoteGitSetupError::Unavailable)),
    ] {
        let value = json!({"version": 1, "state": state, "observation": null});
        assert_eq!(decode(&value, code, false), expected);
        for wrong in [0, 1, 2, 3, 255].into_iter().filter(|wrong| *wrong != code) {
            assert_eq!(decode(&value, wrong, false), Err(RemoteGitSetupError::OutcomeUnknown));
        }
        let extra = json!({"version": 1, "state": state, "observation": observation("complete", None)});
        assert_eq!(decode(&extra, code, false), Err(RemoteGitSetupError::OutcomeUnknown));
    }
}

#[test]
fn missing_duplicate_extra_fields_and_untrusted_values_are_rejected() {
    for (value, observe) in [
        (observation("complete", None), true),
        (json!({"version": 1, "state": "submitted", "observation": null}), false),
    ] {
        for key in value.as_object().unwrap().keys() {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert_eq!(
                decode(&missing, 0, observe),
                Err(RemoteGitSetupError::OutcomeUnknown),
                "{key}"
            );
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        let duplicate = String::from_utf8(bytes.clone())
            .unwrap()
            .replacen('{', "{\"version\":1,", 1);
        for invalid in [duplicate.as_bytes().to_vec(), [bytes, b"{}".to_vec()].concat()] {
            assert_eq!(
                response(&invalid, Some(0), observe),
                Err(RemoteGitSetupError::OutcomeUnknown)
            );
        }
        for (key, replacement) in [
            ("version", json!(true)),
            ("version", json!(2)),
            ("state", json!("private-output")),
            ("extra", json!(null)),
        ] {
            let mut changed = value.clone();
            changed[key] = replacement;
            assert_eq!(decode(&changed, 0, observe), Err(RemoteGitSetupError::OutcomeUnknown));
        }
        let oversized = vec![b' '; if observe { STATUS_LIMIT + 1 } else { LAUNCH_LIMIT + 1 }];
        assert_eq!(
            response(&oversized, Some(0), observe),
            Err(RemoteGitSetupError::OutcomeUnknown)
        );
        assert_eq!(response(b"{}", None, observe), Err(RemoteGitSetupError::OutcomeUnknown));
    }
    for (key, replacement) in [
        ("checkout", json!("/private/untrusted")),
        ("reason", json!("private diagnostic")),
    ] {
        let mut changed = observation("complete", None);
        changed[key] = replacement;
        assert_eq!(decode(&changed, 0, true), Err(RemoteGitSetupError::OutcomeUnknown));
    }
    let duplicate_nested = br#"{"version":1,"state":"observed","observation":{"version":1,"state":"complete","reason":null,"reason":null,"checkout":"/workspace/horizon/repository"}}"#;
    assert_eq!(
        response(duplicate_nested, Some(0), false),
        Err(RemoteGitSetupError::OutcomeUnknown)
    );
}

//! Page request to command input, and command output to page reply.
//!
//! Both sides are untrusted: the page message comes from page script and the
//! command output from an external process. Every field is bounded here.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::origin::{CallerOrigin, client_data_json};
use super::{Operation, PageError};

const MAX_CHALLENGE_BYTES: usize = 1024;
const MAX_CREDENTIAL_ID_BYTES: usize = 1023;
const MAX_USER_ID_BYTES: usize = 64;
const MAX_CREDENTIAL_LIST: usize = 64;
const MAX_ALGORITHMS: usize = 16;
const MAX_NAME_CHARS: usize = 256;
const MAX_SIGNATURE_BYTES: usize = 1024;
const MAX_ATTESTATION_BYTES: usize = 64 * 1024;
const MAX_PUBLIC_KEY_BYTES: usize = 2048;
const MAX_TRANSPORTS: usize = 8;
const MIN_AUTHENTICATOR_DATA_BYTES: usize = 37;
/// Bound for a request that waits for a person to approve it.
pub(crate) const MAX_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_ALGORITHMS: [i64; 2] = [-7, -257];

/// Options the page script extracted from `publicKey`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PageOptions {
    challenge: String,
    rp_id: Option<String>,
    rp_name: Option<String>,
    user: Option<PageUser>,
    algorithms: Vec<i64>,
    allow_credentials: Vec<String>,
    exclude_credentials: Vec<String>,
    user_verification: Option<String>,
    resident_key: Option<String>,
    timeout: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PageUser {
    id: String,
    name: String,
    display_name: String,
}

/// A request that passed every host check and is ready for the command.
#[derive(Debug)]
pub(crate) struct Prepared {
    pub(crate) operation: Operation,
    rp_id: String,
    client_data_json: String,
    allow_credentials: Vec<String>,
    pub(crate) input: Vec<u8>,
    pub(crate) timeout: Duration,
}

pub(crate) fn prepare(
    operation: Operation,
    options: &PageOptions,
    origin: &CallerOrigin,
    agent: Option<&str>,
) -> Result<Prepared, PageError> {
    let challenge = canonical_b64u(&options.challenge, 1, MAX_CHALLENGE_BYTES).ok_or_else(PageError::type_error)?;
    let rp_id = origin.rp_id(options.rp_id.as_deref())?;
    let client_data_json = URL_SAFE_NO_PAD.encode(client_data_json(operation, &challenge, origin));
    let allow_credentials = match operation {
        Operation::Get => credential_list(&options.allow_credentials)?,
        Operation::Create => Vec::new(),
    };
    let user_verification = one_of(
        options.user_verification.as_deref(),
        &["required", "preferred", "discouraged"],
        "preferred",
    );
    let mut requester = json!({ "app": "Horizon" });
    if let Some(agent) = agent.filter(|agent| !agent.is_empty()) {
        requester["agent"] = Value::String(bounded_text(agent));
    }
    let mut input = json!({
        "version": 1,
        "op": operation.as_str(),
        "rp_id": rp_id,
        "origin": origin.as_str(),
        "client_data_json": client_data_json,
        "user_verification": user_verification,
        "requester": requester,
    });
    match operation {
        Operation::Get => {
            input["allow_credentials"] = json!(allow_credentials);
            input["reason"] = Value::String(format!("Sign in to {rp_id} in a browser panel"));
        }
        Operation::Create => {
            let user = options.user.as_ref().ok_or_else(PageError::type_error)?;
            let user_id = canonical_b64u(&user.id, 1, MAX_USER_ID_BYTES).ok_or_else(PageError::type_error)?;
            let algorithms = if options.algorithms.is_empty() {
                DEFAULT_ALGORITHMS.to_vec()
            } else if options.algorithms.len() > MAX_ALGORITHMS {
                return Err(PageError::type_error());
            } else {
                options.algorithms.clone()
            };
            input["rp_name"] = Value::String(bounded_text(options.rp_name.as_deref().unwrap_or(&rp_id)));
            input["user"] = json!({
                "id": user_id,
                "name": bounded_text(&user.name),
                "display_name": bounded_text(&user.display_name),
            });
            input["algorithms"] = json!(algorithms);
            input["exclude_credentials"] = json!(credential_list(&options.exclude_credentials)?);
            input["resident_key"] = Value::String(
                one_of(
                    options.resident_key.as_deref(),
                    &["discouraged", "preferred", "required"],
                    "discouraged",
                )
                .to_string(),
            );
            input["reason"] = Value::String(format!("Create a passkey for {rp_id} in a browser panel"));
        }
    }
    let timeout = options.timeout.map_or(MAX_REQUEST_TIMEOUT, |millis| {
        Duration::from_millis(millis).clamp(MIN_REQUEST_TIMEOUT, MAX_REQUEST_TIMEOUT)
    });
    Ok(Prepared {
        operation,
        rp_id,
        client_data_json,
        allow_credentials,
        input: serde_json::to_vec(&input).map_err(|_| PageError::not_allowed())?,
        timeout,
    })
}

/// Turn the command's stdout into the credential the page receives. The
/// page gets the host's `clientDataJSON`, never a value from the command.
pub(crate) fn credential_from_output(prepared: &Prepared, stdout: &[u8]) -> Result<Value, PageError> {
    let output: Value = serde_json::from_slice(stdout).map_err(|_| refused("the command printed no JSON object"))?;
    if output.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(refused("the command answered with an unsupported version"));
    }
    match output.get("status").and_then(Value::as_str) {
        Some("ok") => {}
        Some("excluded") if prepared.operation == Operation::Create => return Err(PageError::excluded()),
        Some(status) => {
            let message = output.get("message").and_then(Value::as_str).unwrap_or_default();
            tracing::debug!(target: "browser", status, message = %bounded_text(message), "passkey request was not completed");
            return Err(PageError::not_allowed());
        }
        None => return Err(refused("the command answered without a status")),
    }
    let credential_id = required_b64u(&output, "credential_id", MAX_CREDENTIAL_ID_BYTES)?;
    if !prepared.allow_credentials.is_empty() && !prepared.allow_credentials.contains(&credential_id) {
        return Err(refused("the command used a credential that the page did not allow"));
    }
    let authenticator_data = required_b64u(&output, "authenticator_data", MAX_ATTESTATION_BYTES)?;
    check_authenticator_data(&authenticator_data, &prepared.rp_id)?;
    let mut credential = json!({
        "credential_id": credential_id,
        "client_data_json": prepared.client_data_json,
        "authenticator_data": authenticator_data,
    });
    match prepared.operation {
        Operation::Get => {
            credential["signature"] = Value::String(required_b64u(&output, "signature", MAX_SIGNATURE_BYTES)?);
            credential["user_handle"] =
                optional_b64u(&output, "user_handle", MAX_USER_ID_BYTES)?.map_or(Value::Null, Value::String);
        }
        Operation::Create => {
            credential["attestation_object"] =
                Value::String(required_b64u(&output, "attestation_object", MAX_ATTESTATION_BYTES)?);
            credential["public_key"] =
                optional_b64u(&output, "public_key", MAX_PUBLIC_KEY_BYTES)?.map_or(Value::Null, Value::String);
            credential["public_key_algorithm"] = output
                .get("public_key_algorithm")
                .and_then(Value::as_i64)
                .map(Value::from)
                .ok_or_else(|| refused("the command omitted public_key_algorithm"))?;
            credential["transports"] = json!(transports(&output)?);
        }
    }
    Ok(credential)
}

fn check_authenticator_data(encoded: &str, rp_id: &str) -> Result<(), PageError> {
    let data = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| refused("authenticator_data is not base64url"))?;
    if data.len() < MIN_AUTHENTICATOR_DATA_BYTES {
        return Err(refused("authenticator_data is too short"));
    }
    if data[..32] != Sha256::digest(rp_id.as_bytes())[..] {
        return Err(refused("authenticator_data names another relying party"));
    }
    Ok(())
}

fn transports(output: &Value) -> Result<Vec<String>, PageError> {
    let Some(values) = output.get("transports") else {
        return Ok(Vec::new());
    };
    let values = values
        .as_array()
        .filter(|values| values.len() <= MAX_TRANSPORTS)
        .ok_or_else(|| refused("transports is not a short list"))?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|text| {
                    !text.is_empty()
                        && text.len() <= 32
                        && text.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'-')
                })
                .map(str::to_string)
                .ok_or_else(|| refused("transports holds an invalid name"))
        })
        .collect()
}

fn required_b64u(output: &Value, field: &str, max_bytes: usize) -> Result<String, PageError> {
    optional_b64u(output, field, max_bytes)?.ok_or_else(|| refused("the command omitted a required field"))
}

fn optional_b64u(output: &Value, field: &str, max_bytes: usize) -> Result<Option<String>, PageError> {
    match output.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => canonical_b64u(text, 1, max_bytes)
            .map(Some)
            .ok_or_else(|| refused("the command returned a field that is not bounded base64url")),
        Some(_) => Err(refused("the command returned a field that is not a string")),
    }
}

fn credential_list(values: &[String]) -> Result<Vec<String>, PageError> {
    if values.len() > MAX_CREDENTIAL_LIST {
        return Err(PageError::type_error());
    }
    values
        .iter()
        .map(|value| canonical_b64u(value, 1, MAX_CREDENTIAL_ID_BYTES).ok_or_else(PageError::type_error))
        .collect()
}

/// Decode base64url (padding tolerated) within the byte bounds and return
/// the unpadded encoding, so equal bytes always compare equal.
fn canonical_b64u(text: &str, min_bytes: usize, max_bytes: usize) -> Option<String> {
    if text.len() > max_bytes.div_ceil(3) * 4 + 2 {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok()?;
    (min_bytes..=max_bytes)
        .contains(&bytes.len())
        .then(|| URL_SAFE_NO_PAD.encode(bytes))
}

fn one_of<'a>(value: Option<&'a str>, allowed: &[&'static str], default: &'static str) -> &'a str {
    value.filter(|value| allowed.contains(value)).unwrap_or(default)
}

fn bounded_text(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control())
        .take(MAX_NAME_CHARS)
        .collect()
}

fn refused(reason: &str) -> PageError {
    tracing::warn!(target: "browser", "passkey authenticator command answer refused: {reason}");
    PageError::not_allowed()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> CallerOrigin {
        CallerOrigin::parse("https://login.example.com").expect("origin")
    }

    fn get_options(challenge: &str) -> PageOptions {
        PageOptions {
            challenge: challenge.to_string(),
            allow_credentials: vec!["AQID".into()],
            timeout: Some(600_000),
            ..PageOptions::default()
        }
    }

    fn authenticator_data(rp_id: &str) -> String {
        let mut data = Sha256::digest(rp_id.as_bytes()).to_vec();
        data.extend([0x05, 0, 0, 0, 1]);
        URL_SAFE_NO_PAD.encode(data)
    }

    #[test]
    fn get_input_follows_the_command_contract() {
        let prepared = prepare(
            Operation::Get,
            &get_options("AAECAwQFBgcICQoLDA0ODw=="),
            &origin(),
            Some("agent"),
        )
        .expect("prepared");
        let input: Value = serde_json::from_slice(&prepared.input).expect("json");
        assert_eq!(input["version"], 1);
        assert_eq!(input["op"], "get");
        assert_eq!(input["rp_id"], "login.example.com");
        assert_eq!(input["origin"], "https://login.example.com");
        assert_eq!(input["allow_credentials"], json!(["AQID"]));
        assert_eq!(input["user_verification"], "preferred");
        assert_eq!(input["requester"], json!({"app": "Horizon", "agent": "agent"}));
        assert_eq!(input["reason"], "Sign in to login.example.com in a browser panel");
        let client_data = URL_SAFE_NO_PAD
            .decode(input["client_data_json"].as_str().expect("client data"))
            .expect("b64u");
        let client_data: Value = serde_json::from_slice(&client_data).expect("client data json");
        assert_eq!(client_data["challenge"], "AAECAwQFBgcICQoLDA0ODw");
        assert_eq!(client_data["origin"], "https://login.example.com");
        assert_eq!(client_data["crossOrigin"], false);
        assert_eq!(prepared.timeout, MAX_REQUEST_TIMEOUT);
    }

    #[test]
    fn create_input_carries_user_algorithms_and_resident_key() {
        let options = PageOptions {
            challenge: "AAECAwQFBgcICQoLDA0ODw".into(),
            rp_id: Some("example.com".into()),
            rp_name: Some("Example".into()),
            user: Some(PageUser {
                id: "dXNlcg".into(),
                name: "user@example.com".into(),
                display_name: "User\u{7}".into(),
            }),
            resident_key: Some("required".into()),
            user_verification: Some("bogus".into()),
            timeout: Some(1),
            ..PageOptions::default()
        };
        let prepared = prepare(Operation::Create, &options, &origin(), None).expect("prepared");
        let input: Value = serde_json::from_slice(&prepared.input).expect("json");
        assert_eq!(input["rp_id"], "example.com");
        assert_eq!(input["rp_name"], "Example");
        assert_eq!(
            input["user"],
            json!({"id": "dXNlcg", "name": "user@example.com", "display_name": "User"})
        );
        assert_eq!(input["algorithms"], json!([-7, -257]));
        assert_eq!(input["exclude_credentials"], json!([]));
        assert_eq!(input["resident_key"], "required");
        assert_eq!(input["user_verification"], "preferred");
        assert_eq!(input["requester"], json!({"app": "Horizon"}));
        assert_eq!(prepared.timeout, MIN_REQUEST_TIMEOUT);
    }

    #[test]
    fn malformed_page_options_are_type_errors() {
        for options in [
            get_options(""),
            get_options("not base64!"),
            get_options(&"A".repeat(4096)),
            PageOptions {
                allow_credentials: vec!["AQID".into(); MAX_CREDENTIAL_LIST + 1],
                ..get_options("AAEC")
            },
        ] {
            assert_eq!(
                prepare(Operation::Get, &options, &origin(), None)
                    .expect_err("refused")
                    .name,
                "TypeError"
            );
        }
        let no_user = get_options("AAEC");
        assert_eq!(
            prepare(Operation::Create, &no_user, &origin(), None)
                .expect_err("user")
                .name,
            "TypeError"
        );
    }

    #[test]
    fn assertion_output_is_bounded_and_bound_to_the_rp_and_allow_list() {
        let prepared = prepare(Operation::Get, &get_options("AAEC"), &origin(), None).expect("prepared");
        let ok = json!({
            "version": 1, "status": "ok", "credential_id": "AQID",
            "authenticator_data": authenticator_data("login.example.com"),
            "signature": "MEUCIQ", "user_handle": "dXNlcg",
            "client_data_json": "ignored-from-command"
        });
        let credential = credential_from_output(&prepared, ok.to_string().as_bytes()).expect("credential");
        assert_eq!(
            credential["client_data_json"],
            Value::String(prepared.client_data_json.clone())
        );
        assert_eq!(credential["user_handle"], "dXNlcg");

        let mut other_rp = ok.clone();
        other_rp["authenticator_data"] = Value::String(authenticator_data("example.org"));
        let mut not_allowed = ok.clone();
        not_allowed["credential_id"] = Value::String("BAUG".into());
        let mut missing_signature = ok.clone();
        missing_signature.as_object_mut().expect("object").remove("signature");
        for output in [
            other_rp,
            not_allowed,
            missing_signature,
            json!({"version": 2, "status": "ok"}),
        ] {
            let error = credential_from_output(&prepared, output.to_string().as_bytes()).expect_err("refused");
            assert_eq!(error.name, "NotAllowedError");
        }
        assert_eq!(
            credential_from_output(&prepared, b"not json").expect_err("json").name,
            "NotAllowedError"
        );
    }

    #[test]
    fn statuses_map_to_indistinguishable_page_errors_except_excluded_create() {
        let get = prepare(Operation::Get, &get_options("AAEC"), &origin(), None).expect("get");
        for status in ["denied", "expired", "not_found", "error", "excluded"] {
            let output = json!({"version": 1, "status": status, "message": "detail"}).to_string();
            let error = credential_from_output(&get, output.as_bytes()).expect_err(status);
            assert_eq!(
                (error.name, error.message),
                (PageError::not_allowed().name, PageError::not_allowed().message)
            );
        }
        let create_options = PageOptions {
            user: Some(PageUser {
                id: "dXNlcg".into(),
                ..PageUser::default()
            }),
            ..get_options("AAEC")
        };
        let create = prepare(Operation::Create, &create_options, &origin(), None).expect("create");
        let excluded = json!({"version": 1, "status": "excluded"}).to_string();
        assert_eq!(
            credential_from_output(&create, excluded.as_bytes())
                .expect_err("excluded")
                .name,
            "InvalidStateError"
        );
    }

    #[test]
    fn attestation_output_keeps_key_algorithm_and_transports() {
        let options = PageOptions {
            user: Some(PageUser {
                id: "dXNlcg".into(),
                ..PageUser::default()
            }),
            ..get_options("AAEC")
        };
        let prepared = prepare(Operation::Create, &options, &origin(), None).expect("prepared");
        let output = json!({
            "version": 1, "status": "ok", "credential_id": "AQID",
            "attestation_object": "oA", "authenticator_data": authenticator_data("login.example.com"),
            "public_key": "MFk", "public_key_algorithm": -7, "transports": ["internal", "hybrid"]
        });
        let credential = credential_from_output(&prepared, output.to_string().as_bytes()).expect("credential");
        assert_eq!(credential["public_key_algorithm"], -7);
        assert_eq!(credential["transports"], json!(["internal", "hybrid"]));
        let mut bad = output.clone();
        bad["transports"] = json!(["<script>"]);
        assert!(credential_from_output(&prepared, bad.to_string().as_bytes()).is_err());
    }
}

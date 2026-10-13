//! Fake external authenticator command with test P-256 keys. It follows the
//! bridge's stdin/stdout contract and keeps its keys in a state directory.
//! An optional `mode` file in that directory changes the answer: `hang`
//! records the PID and waits to be killed, `slow` answers after three
//! seconds, and `deny` answers `denied`.

use std::io::{Read, Write};
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SPKI_P256_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce,
    0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

pub(crate) fn run(state: &Path) -> i32 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return 1;
    }
    let Ok(request) = serde_json::from_str::<Value>(&input) else {
        return 1;
    };
    let op = request["op"].as_str().unwrap_or_default();
    if let Ok(mut log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join("calls.log"))
    {
        let _ = writeln!(log, "{op} {}", request["origin"].as_str().unwrap_or_default());
    }
    match std::fs::read_to_string(state.join("mode")).unwrap_or_default().trim() {
        "hang" => {
            let _ = std::fs::write(state.join("hang.pid"), std::process::id().to_string());
            std::thread::sleep(std::time::Duration::from_secs(120));
            return 0;
        }
        "slow" => std::thread::sleep(std::time::Duration::from_secs(3)),
        "deny" => {
            println!("{}", json!({ "version": 1, "status": "denied", "message": "denied by the test" }));
            return 0;
        }
        _ => {}
    }
    let answer = match op {
        "create" => create(state, &request),
        "get" => get(state, &request),
        _ => Err("unknown op".to_string()),
    }
    .unwrap_or_else(|message| json!({ "version": 1, "status": "error", "message": message }));
    println!("{answer}");
    0
}

fn create(state: &Path, request: &Value) -> Result<Value, String> {
    let rp_id = request["rp_id"].as_str().ok_or("rp_id")?;
    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).map_err(|_| "keygen")?;
    let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng).map_err(|_| "key")?;
    let mut credential_id = [0_u8; 16];
    rng.fill(&mut credential_id).map_err(|_| "random")?;
    let id = URL_SAFE_NO_PAD.encode(credential_id);
    let user_handle = request["user"]["id"].as_str().ok_or("user id")?;
    std::fs::write(state.join(format!("{id}.key")), pkcs8.as_ref()).map_err(|error| error.to_string())?;
    std::fs::write(state.join(format!("{id}.user")), user_handle).map_err(|error| error.to_string())?;

    let point = key.public_key().as_ref();
    let mut auth_data = Sha256::digest(rp_id.as_bytes()).to_vec();
    auth_data.push(0x45);
    auth_data.extend([0, 0, 0, 0]);
    auth_data.extend([0; 16]);
    auth_data.extend(u16::try_from(credential_id.len()).map_err(|_| "id length")?.to_be_bytes());
    auth_data.extend(credential_id);
    auth_data.extend([0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21, 0x58, 0x20]);
    auth_data.extend(&point[1..33]);
    auth_data.extend([0x22, 0x58, 0x20]);
    auth_data.extend(&point[33..65]);

    let mut attestation = vec![0xa3, 0x63];
    attestation.extend(b"fmt");
    attestation.extend([0x64]);
    attestation.extend(b"none");
    attestation.extend([0x67]);
    attestation.extend(b"attStmt");
    attestation.extend([0xa0, 0x68]);
    attestation.extend(b"authData");
    attestation.extend([0x59]);
    attestation.extend(u16::try_from(auth_data.len()).map_err(|_| "authData length")?.to_be_bytes());
    attestation.extend(&auth_data);

    let mut spki = SPKI_P256_PREFIX.to_vec();
    spki.extend(point);
    Ok(json!({
        "version": 1,
        "status": "ok",
        "credential_id": id,
        "attestation_object": URL_SAFE_NO_PAD.encode(attestation),
        "authenticator_data": URL_SAFE_NO_PAD.encode(auth_data),
        "public_key": URL_SAFE_NO_PAD.encode(spki),
        "public_key_algorithm": -7,
        "transports": ["internal"],
    }))
}

fn get(state: &Path, request: &Value) -> Result<Value, String> {
    let rp_id = request["rp_id"].as_str().ok_or("rp_id")?;
    let client_data = URL_SAFE_NO_PAD
        .decode(request["client_data_json"].as_str().ok_or("client data")?)
        .map_err(|error| error.to_string())?;
    let allowed: Vec<String> = request["allow_credentials"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    // An empty allow list asks for a discoverable credential: use any key.
    let candidates = if allowed.is_empty() { stored_ids(state) } else { allowed };
    let Some((id, pkcs8)) = candidates
        .into_iter()
        .find_map(|id| std::fs::read(state.join(format!("{id}.key"))).ok().map(|key| (id, key)))
    else {
        return Ok(json!({ "version": 1, "status": "not_found", "message": "no matching credential" }));
    };
    let rng = SystemRandom::new();
    let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &pkcs8, &rng).map_err(|_| "key")?;
    let mut auth_data = Sha256::digest(rp_id.as_bytes()).to_vec();
    auth_data.push(0x05);
    auth_data.extend([0, 0, 0, 1]);
    let mut signed = auth_data.clone();
    signed.extend_from_slice(&Sha256::digest(&client_data));
    let signature = key.sign(&rng, &signed).map_err(|_| "sign")?;
    let user_handle = std::fs::read_to_string(state.join(format!("{id}.user"))).ok();
    Ok(json!({
        "version": 1,
        "status": "ok",
        "credential_id": id,
        "authenticator_data": URL_SAFE_NO_PAD.encode(auth_data),
        "signature": URL_SAFE_NO_PAD.encode(signature.as_ref()),
        "user_handle": user_handle,
    }))
}

fn stored_ids(state: &Path) -> Vec<String> {
    std::fs::read_dir(state)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.strip_suffix(".key").map(str::to_string))
        .collect()
}

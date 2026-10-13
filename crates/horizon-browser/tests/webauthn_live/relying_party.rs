//! Local relying party for the live `WebAuthn` bridge test. It serves one
//! page per scenario and verifies created credentials and assertions.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use horizon_browser::BrowserSession;
use ring::rand::SecureRandom;
use serde_json::Value;
use sha2::{Digest, Sha256};

const SPKI_P256_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce,
    0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];
const USER_ID: &str = "AQIDBA";

const PRELUDE: &str = r#"
const b64u = (buffer) => btoa(String.fromCharCode(...new Uint8Array(buffer))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const unb64u = (text) => Uint8Array.from(atob(text.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));
const report = (scenario, body) => fetch("/report", { method: "POST", body: JSON.stringify({ scenario, ...body }) });
const outcome = async (scenario, promise) => {
    try { await promise; await report(scenario, { verdict: "resolved" }); }
    catch (error) { await report(scenario, { verdict: error.name, message: String(error.message) }); }
};
const anyChallenge = () => crypto.getRandomValues(new Uint8Array(32));
"#;

#[derive(Debug)]
pub(crate) struct Report {
    pub(crate) scenario: String,
    pub(crate) verdict: String,
}

#[derive(Default)]
struct State {
    challenge: String,
    credential: Option<(String, Vec<u8>)>,
}

struct Shared {
    port: u16,
    frame_port: u16,
    state: Mutex<State>,
    abort: AtomicBool,
    reports: Mutex<mpsc::Sender<Report>>,
}

pub(crate) struct RelyingParty {
    shared: Arc<Shared>,
    reports: mpsc::Receiver<Report>,
}

impl RelyingParty {
    pub(crate) fn start() -> Self {
        let main = TcpListener::bind("127.0.0.1:0").expect("bind relying party");
        let frame = TcpListener::bind("127.0.0.1:0").expect("bind frame origin");
        let (sender, reports) = mpsc::channel();
        let shared = Arc::new(Shared {
            port: main.local_addr().expect("addr").port(),
            frame_port: frame.local_addr().expect("addr").port(),
            state: Mutex::new(State::default()),
            abort: AtomicBool::new(false),
            reports: Mutex::new(sender),
        });
        for listener in [main, frame] {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let shared = Arc::clone(&shared);
                    std::thread::spawn(move || serve(&shared, stream));
                }
            });
        }
        Self { shared, reports }
    }

    pub(crate) fn url(&self, path: &str) -> String {
        format!("http://localhost:{}{path}", self.shared.port)
    }

    pub(crate) fn insecure_url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.shared.port)
    }

    pub(crate) fn release_abort(&self) {
        self.shared.abort.store(true, Ordering::Release);
    }

    pub(crate) fn wait_report(&self, scenario: &str, timeout: Duration, session: &BrowserSession) -> Report {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            super::drain(session);
            match self.reports.recv_timeout(Duration::from_millis(100)) {
                Ok(report) if report.scenario == scenario => return report,
                Ok(report) => println!("ignoring report {report:?} while waiting for {scenario}"),
                Err(_) => {}
            }
        }
        panic!("no {scenario} report");
    }
}

fn serve(shared: &Shared, stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut content_length = 0;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; content_length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let (content_type, response) = match path {
        "/report" => ("text/plain", handle_report(shared, &body)),
        "/abort-now" => (
            "text/plain",
            if shared.abort.load(Ordering::Acquire) { "1" } else { "0" }.to_string(),
        ),
        "/favicon.ico" => ("text/plain", String::new()),
        page => ("text/html; charset=utf-8", page_html(shared, page)),
    };
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{response}",
        response.len()
    );
}

fn page_html(shared: &Shared, page: &str) -> String {
    let script = match page {
        "/create" => {
            let challenge = new_challenge(shared);
            format!(
                r#"(async () => {{
    try {{
        const credential = await navigator.credentials.create({{ publicKey: {{
            challenge: unb64u("{challenge}"),
            rp: {{ name: "Horizon test relying party" }},
            user: {{ id: unb64u("{USER_ID}"), name: "user@localhost", displayName: "Test user" }},
            pubKeyCredParams: [{{ type: "public-key", alg: -7 }}],
            authenticatorSelection: {{ residentKey: "required", userVerification: "preferred" }},
            timeout: 60000,
        }} }});
        await report("create", {{
            instance: credential instanceof PublicKeyCredential,
            id: credential.id,
            rawId: b64u(credential.rawId),
            attachment: credential.authenticatorAttachment,
            publicKey: b64u(credential.response.getPublicKey()),
            algorithm: credential.response.getPublicKeyAlgorithm(),
            transports: credential.response.getTransports(),
            authenticatorData: b64u(credential.response.getAuthenticatorData()),
            clientDataJSON: b64u(credential.response.clientDataJSON),
            attestationObject: b64u(credential.response.attestationObject),
            json: credential.toJSON(),
        }});
    }} catch (error) {{ await report("create", {{ error: error.name + ": " + error.message }}); }}
}})();"#
            )
        }
        "/get" => {
            let challenge = new_challenge(shared);
            let credential = lock(shared).credential.clone().map(|(id, _)| id).unwrap_or_default();
            format!(
                r#"(async () => {{
    try {{
        const credential = await navigator.credentials.get({{ publicKey: {{
            challenge: unb64u("{challenge}"),
            rpId: "localhost",
            allowCredentials: [{{ type: "public-key", id: unb64u("{credential}") }}],
            userVerification: "preferred",
            timeout: 60000,
        }} }});
        await report("get", {{
            instance: credential instanceof PublicKeyCredential,
            id: credential.id,
            authenticatorData: b64u(credential.response.authenticatorData),
            clientDataJSON: b64u(credential.response.clientDataJSON),
            signature: b64u(credential.response.signature),
            userHandle: credential.response.userHandle ? b64u(credential.response.userHandle) : null,
            json: credential.toJSON(),
        }});
    }} catch (error) {{ await report("get", {{ error: error.name + ": " + error.message }}); }}
}})();"#
            )
        }
        "/refusals" => format!(
            r#"(async () => {{
    await outcome("foreign_rp_id", navigator.credentials.get({{ publicKey: {{ challenge: anyChallenge(), rpId: "example.com" }} }}));
    await outcome("malformed_options", navigator.credentials.create({{ publicKey: {{
        challenge: anyChallenge(), rp: {{ name: "x" }}, user: {{ id: anyChallenge(), name: "u", displayName: "u" }},
        pubKeyCredParams: [{{ type: "public-key" }}],
    }} }}));
    await report("conditional_available", {{ verdict: String(await PublicKeyCredential.isConditionalMediationAvailable()) }});
    const controller = new AbortController();
    const conditional = navigator.credentials.get({{ mediation: "conditional", publicKey: {{ challenge: anyChallenge() }}, signal: controller.signal }});
    setTimeout(() => controller.abort(), 300);
    await outcome("conditional", conditional);
    const frame = document.createElement("iframe");
    frame.src = "http://localhost:{}/frame";
    document.body.appendChild(frame);
}})();"#,
            shared.frame_port
        ),
        "/frame" => r#"outcome("cross_origin_iframe", navigator.credentials.get({ publicKey: { challenge: anyChallenge() } }));"#
            .to_string(),
        "/insecure" => r#"outcome("insecure_origin", navigator.credentials.get({ publicKey: { challenge: anyChallenge() } }));"#
            .to_string(),
        "/abort" => r#"const controller = new AbortController();
const pending = navigator.credentials.get({ publicKey: { challenge: anyChallenge() }, signal: controller.signal });
(async () => {
    while ((await (await fetch("/abort-now")).text()) !== "1") await new Promise((done) => setTimeout(done, 100));
    controller.abort();
})();
outcome("abort", pending);"#
            .to_string(),
        _ => String::new(),
    };
    format!(
        "<!doctype html><title>passkey bridge test</title><body><p>{page}</p><script>{PRELUDE}\n{script}</script></body>"
    )
}

fn handle_report(shared: &Shared, body: &[u8]) -> String {
    let Ok(report) = serde_json::from_slice::<Value>(body) else {
        return "bad".to_string();
    };
    let scenario = report["scenario"].as_str().unwrap_or_default().to_string();
    let verdict = match scenario.as_str() {
        "create" => {
            verify_create(shared, &report).map_or_else(|reason| format!("failed: {reason}"), |()| "verified".into())
        }
        "get" => verify_get(shared, &report).map_or_else(|reason| format!("failed: {reason}"), |()| "verified".into()),
        _ => report["verdict"].as_str().unwrap_or_default().to_string(),
    };
    let sender = shared.reports.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let _ = sender.send(Report { scenario, verdict });
    "ok".to_string()
}

fn verify_create(shared: &Shared, report: &Value) -> Result<(), String> {
    if let Some(error) = report["error"].as_str() {
        return Err(error.to_string());
    }
    expect(report["instance"] == true, "not a PublicKeyCredential")?;
    expect(report["attachment"] == "platform", "authenticatorAttachment")?;
    expect(report["algorithm"] == -7, "public key algorithm")?;
    expect(report["id"] == report["rawId"], "id and rawId differ")?;
    let client_data = client_data(shared, report, "webauthn.create")?;
    let auth_data = decode(&report["authenticatorData"])?;
    check_rp_hash(&auth_data)?;
    expect(
        auth_data[32] & 0x41 == 0x41,
        "user presence or attested data flag missing",
    )?;
    let id_length = usize::from(u16::from_be_bytes([auth_data[53], auth_data[54]]));
    let credential_id = auth_data.get(55..55 + id_length).ok_or("short attested data")?;
    expect(
        URL_SAFE_NO_PAD.encode(credential_id) == report["rawId"].as_str().unwrap_or_default(),
        "credential id",
    )?;
    let spki = decode(&report["publicKey"])?;
    expect(
        spki.len() == 91 && spki[..26] == SPKI_P256_PREFIX,
        "public key is not a P-256 SPKI",
    )?;
    expect(
        decode(&report["attestationObject"])?.first() == Some(&0xa3),
        "attestation object is not a CBOR map",
    )?;
    let json = &report["json"];
    expect(
        json["id"] == report["id"] && json["type"] == "public-key",
        "toJSON identity",
    )?;
    expect(
        json["response"]["clientDataJSON"] == report["clientDataJSON"],
        "toJSON client data",
    )?;
    expect(json["response"]["publicKeyAlgorithm"] == -7, "toJSON algorithm")?;
    expect(!client_data.is_empty(), "client data")?;
    lock(shared).credential = Some((
        report["id"].as_str().unwrap_or_default().to_string(),
        spki[26..].to_vec(),
    ));
    Ok(())
}

fn verify_get(shared: &Shared, report: &Value) -> Result<(), String> {
    if let Some(error) = report["error"].as_str() {
        return Err(error.to_string());
    }
    expect(report["instance"] == true, "not a PublicKeyCredential")?;
    let client_data = client_data(shared, report, "webauthn.get")?;
    let (credential_id, public_key) = lock(shared).credential.clone().ok_or("no registered credential")?;
    expect(report["id"] == credential_id.as_str(), "unexpected credential")?;
    expect(report["userHandle"] == USER_ID, "user handle")?;
    let auth_data = decode(&report["authenticatorData"])?;
    check_rp_hash(&auth_data)?;
    expect(auth_data[32] & 0x01 == 0x01, "user presence flag missing")?;
    let mut signed = auth_data.clone();
    signed.extend_from_slice(&Sha256::digest(&client_data));
    let signature = decode(&report["signature"])?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, public_key)
        .verify(&signed, &signature)
        .map_err(|_| "signature does not verify".to_string())?;
    expect(
        report["json"]["response"]["signature"] == report["signature"],
        "toJSON signature",
    )
}

fn client_data(shared: &Shared, report: &Value, kind: &str) -> Result<Vec<u8>, String> {
    let bytes = decode(&report["clientDataJSON"])?;
    let parsed: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let challenge = lock(shared).challenge.clone();
    expect(parsed["type"] == kind, "client data type")?;
    expect(parsed["challenge"] == challenge.as_str(), "client data challenge")?;
    expect(
        parsed["origin"] == format!("http://localhost:{}", shared.port).as_str(),
        "client data origin",
    )?;
    expect(parsed["crossOrigin"] == false, "client data crossOrigin")?;
    Ok(bytes)
}

fn check_rp_hash(auth_data: &[u8]) -> Result<(), String> {
    expect(auth_data.len() >= 37, "short authenticator data")?;
    expect(auth_data[..32] == Sha256::digest(b"localhost")[..], "rpIdHash")
}

fn decode(value: &Value) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD
        .decode(value.as_str().ok_or("missing field")?)
        .map_err(|error| error.to_string())
}

fn expect(condition: bool, what: &str) -> Result<(), String> {
    if condition { Ok(()) } else { Err(what.to_string()) }
}

fn new_challenge(shared: &Shared) -> String {
    let mut bytes = [0_u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .expect("random challenge");
    let challenge = URL_SAFE_NO_PAD.encode(bytes);
    lock(shared).challenge.clone_from(&challenge);
    challenge
}

fn lock(shared: &Shared) -> std::sync::MutexGuard<'_, State> {
    shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

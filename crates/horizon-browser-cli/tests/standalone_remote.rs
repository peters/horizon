//! A standalone remote host against a loopback `WebDriver` provider: it opens a
//! session for a configured target, answers usage and allocations, releases
//! the session on close and on stdin end, and never shows a credential value.
// Unix only: the host checks the modes of the host file and the credential files, and the
// recovery test kills the host with SIGKILL.
#![cfg(unix)]

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const USER: &str = "fixture-user";
const KEY: &str = "fixture-key-0d9f";
/// A 1x1 PNG.
const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    authorization: Option<String>,
}

/// A loopback provider. Sessions in `ended` answer as gone, as after the
/// provider's own idle timeout.
struct Provider {
    endpoint: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    ended: Arc<Mutex<BTreeSet<String>>>,
}

fn provider() -> Provider {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind provider");
    let endpoint = format!("http://{}/wd/hub", listener.local_addr().expect("address"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let ended = Arc::new(Mutex::new(BTreeSet::new()));
    let (record, gone) = (Arc::clone(&seen), Arc::clone(&ended));
    std::thread::spawn(move || {
        let mut sessions = 0;
        for stream in listener.incoming().flatten() {
            let Some(request) = read_request(&stream) else { continue };
            let (status, body) = answer(&request, &mut sessions, &mut gone.lock().expect("ended"));
            record.lock().expect("record").push(request);
            let body = body.to_string();
            let _ = write!(
                &stream,
                "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    Provider { endpoint, seen, ended }
}

fn read_request(stream: &TcpStream) -> Option<Seen> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut length = 0;
    let mut authorization = None;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        match name.to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().ok()?,
            "authorization" => authorization = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(Seen {
        method,
        path,
        authorization,
    })
}

fn answer(request: &Seen, sessions: &mut u32, ended: &mut BTreeSet<String>) -> (u16, Value) {
    let path = request.path.as_str();
    if request.method == "POST" && path == "/wd/hub/session" {
        *sessions += 1;
        return (
            200,
            json!({"value": {"sessionId": format!("fixture-{sessions}"), "capabilities": {
                "browserName": "chrome", "platformName": "linux", "browserVersion": "1"
            }}}),
        );
    }
    let session = path
        .strip_prefix("/wd/hub/session/")
        .map(|rest| rest.split('/').next().unwrap_or_default().to_owned());
    if let Some(session) = session {
        if ended.contains(&session) {
            return (
                404,
                json!({"value": {"error": "invalid session id", "message": "gone"}}),
            );
        }
        if request.method == "DELETE" && !path.ends_with("/window") {
            ended.insert(session);
        }
    }
    let value = if path.ends_with("/screenshot") {
        json!(PIXEL)
    } else if path.ends_with("/window/rect") {
        json!({"x": 0, "y": 0, "width": 1280, "height": 800})
    } else if path.ends_with("/window/handles") {
        json!(["window-1"])
    } else if path.ends_with("/window") && request.method == "GET" {
        json!("window-1")
    } else if path.ends_with("/url") && request.method == "GET" {
        json!("about:blank")
    } else if path.ends_with("/title") {
        json!("")
    } else {
        Value::Null
    };
    (200, json!({ "value": value }))
}

fn private(path: &Path, text: &str, mode: u32) {
    std::fs::write(path, text).expect("write");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("mode");
}

struct Host {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Host {
    fn start(root: &Path, endpoint: &str) -> Self {
        private(&root.join("user"), &format!("{USER}\n"), 0o600);
        private(&root.join("key"), KEY, 0o600);
        let host_file = format!(
            "browser:\n  remote:\n    providers:\n      grid:\n        adapter: webdriver\n        endpoint: {endpoint}\n        \
             authentication: {{ kind: basic, username_ref: user, password_ref: key }}\n    targets:\n      desktop:\n        \
             provider: grid\n        browser_name: chrome\n        platform_name: linux\n        \
             device: {{ kind: any }}\ncredential_files:\n  grid:\n    \
             user: {}\n    key: {}\n",
            root.join("user").display(),
            root.join("key").display()
        );
        private(&root.join("host.yaml"), &host_file, 0o600);
        let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
            .args(["mcp", "--standalone", "--config"])
            .arg(root.join("host.yaml"))
            .env("HOME", root)
            .env("HORIZON_BROWSER_ROOT", root.join(".horizon"))
            .env_remove("HORIZON_BROWSER_ACTOR")
            .env_remove("HORIZON_BROWSER_CONFIG")
            .env("RUST_LOG", "warn")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the standalone host");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut host = Self {
            child,
            stdin,
            stdout,
            next: 1,
        };
        host.call(
            "initialize",
            &json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}),
        );
        host.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        host
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{message}").expect("write");
        stdin.flush().expect("flush");
    }

    fn call(&mut self, method: &str, params: &Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).expect("read") > 0,
                "host closed stdout"
            );
            let message: Value = serde_json::from_str(&line).expect("json-rpc");
            if message["id"] == id {
                return message;
            }
        }
    }

    /// The tool's structured result, or its error text.
    fn tool(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
        let reply = self.call("tools/call", &json!({"name": name, "arguments": arguments}));
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_default().to_owned();
        if result["isError"] == true {
            return Err(text);
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    /// Close stdin and wait for the exit; returns the success and stderr.
    fn finish(mut self) -> (bool, String) {
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(60);
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("wait") {
                break status;
            }
            assert!(Instant::now() < deadline, "the host did not exit after stdin closed");
            std::thread::sleep(Duration::from_millis(50));
        };
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .expect("stderr")
            .read_to_string(&mut stderr)
            .expect("stderr");
        (status.success(), stderr)
    }
}

fn deletes(seen: &Mutex<Vec<Seen>>) -> Vec<Seen> {
    seen.lock()
        .expect("seen")
        .iter()
        .filter(|seen| {
            seen.method == "DELETE"
                && seen
                    .path
                    .strip_prefix("/wd/hub/session/")
                    .is_some_and(|session| !session.contains('/'))
        })
        .cloned()
        .collect()
}

#[test]
fn a_configured_target_opens_answers_and_is_released_on_close_and_on_exit() {
    let root = tempfile::tempdir().expect("root");
    let Provider { endpoint, seen, .. } = provider();
    let mut host = Host::start(root.path(), &endpoint);

    let created = host
        .tool("browser_create", &json!({"target": "desktop"}))
        .expect("create");
    let panel = created["panel"]["panel_id"].as_str().expect("panel id").to_owned();
    let new_session = seen.lock().expect("seen")[0].clone();
    assert_eq!(
        (new_session.method.as_str(), new_session.path.as_str()),
        ("POST", "/wd/hub/session")
    );
    assert_eq!(
        new_session.authorization.as_deref(),
        Some("Basic Zml4dHVyZS11c2VyOmZpeHR1cmUta2V5LTBkOWY="),
        "the header comes from the credential files, without the final line break"
    );
    let listed = host.tool("browser_list", &json!({})).expect("list");
    assert!(listed.to_string().contains(&panel), "{listed}");

    let usage = host.tool("browser_provider_usage", &json!({})).expect("usage");
    assert_eq!(usage["providers"][0]["provider"], "grid", "{usage}");
    assert_eq!(usage["providers"][0]["local_session_limit"], 1, "{usage}");
    let held = host
        .tool("browser_remote_allocations", &json!({"operation": "list"}))
        .expect("allocations");
    assert_eq!(held["allocations"][0]["provider"], "grid", "{held}");

    let refused = host
        .tool("browser_create", &json!({"allow_additional": true}))
        .unwrap_err();
    assert!(refused.contains("target_required"), "{refused}");

    host.tool("browser_close", &json!({"panel_id": panel})).expect("close");
    let released = deletes(&seen);
    assert_eq!(released.len(), 1, "close releases the session once: {released:?}");
    assert_eq!(released[0].path, "/wd/hub/session/fixture-1");
    let after = host
        .tool("browser_remote_allocations", &json!({"operation": "list"}))
        .expect("allocations after close");
    assert_eq!(after["allocations"][0]["status"], "released", "{after}");

    host.tool("browser_create", &json!({"target": "desktop"}))
        .expect("second create");
    let (success, stderr) = host.finish();
    assert!(success, "{stderr}");
    let released = deletes(&seen);
    assert_eq!(released.len(), 2, "stdin end releases the open session: {released:?}");
    assert_eq!(released[1].path, "/wd/hub/session/fixture-2");
    assert!(!stderr.contains(KEY) && !stderr.contains(USER), "{stderr}");
    let holds = root.path().join(".horizon").join("standalone-remote-holds");
    let left: Vec<_> = walk(&holds);
    assert!(
        left.is_empty(),
        "a host that released everything leaves no files: {left:?}"
    );
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn a_host_after_a_killed_host_checks_its_session_at_the_provider() {
    let root = tempfile::tempdir().expect("root");
    let provider = provider();
    let mut first = Host::start(root.path(), &provider.endpoint);
    first
        .tool("browser_create", &json!({"target": "desktop"}))
        .expect("create");
    first.child.kill().expect("kill the first host");
    first.child.wait().expect("reap the first host");

    let mut second = Host::start(root.path(), &provider.endpoint);
    let held = settled(&mut second);
    assert_eq!(held["provider"], "grid", "{held}");
    assert_eq!(held["status"], "active", "the provider still runs the session: {held}");

    // The provider ends the idle session itself; reconciling then releases the hold.
    provider.ended.lock().expect("ended").insert("fixture-1".into());
    let reference = held["reference"].as_str().expect("reference").to_owned();
    second
        .tool(
            "browser_remote_allocations",
            &json!({"operation": "reconcile", "reference": reference}),
        )
        .expect("reconcile");
    let held = settled(&mut second);
    assert_eq!(held["status"], "released", "{held}");
    let (success, stderr) = second.finish();
    assert!(success, "{stderr}");
    let holds = root.path().join(".horizon").join("standalone-remote-holds");
    assert!(walk(&holds).is_empty(), "the released hold leaves no files");
}

/// The first allocation once its check at the provider finished.
fn settled(host: &mut Host) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let listed = host
            .tool("browser_remote_allocations", &json!({"operation": "list"}))
            .expect("allocations");
        let first = listed["allocations"][0].clone();
        if first.is_object() && first["status"] != "reconciling" {
            return first;
        }
        assert!(
            Instant::now() < deadline,
            "the allocation check did not finish: {listed}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

//! A profile host through the binary: it keeps one stable host instance for
//! its root, refuses a second host and a bad host file, answers `--connect`
//! clients that name no profile or a remote target, and keeps its profiles
//! over a termination signal. The live tests open real pages and need a
//! local Chromium; run them with `cargo test -p horizon-browser-cli --test
//! standalone_profile -- --ignored`.
// Unix only: the host checks the mode of its host file, and the tests send SIGTERM.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const HOST_FILE: &str = "browser:\n  profiles:\n    mail:\n      backend: chromium\n      args: [\"--disable-gpu\"]\n";

fn host_file(root: &Path, text: &str) -> PathBuf {
    let path = root.join("host.yaml");
    std::fs::write(&path, text).expect("write host file");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("mode");
    path
}

/// A running profile host. A test that fails before it stops the host
/// still stops it here, so that no host and no browser of the test stays.
struct Host(Option<Child>);

impl Host {
    fn id(&self) -> u32 {
        self.0.as_ref().map_or(0, Child::id)
    }

    fn into_child(mut self) -> Child {
        self.0.take().expect("a running host")
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = Command::new("kill").args(["-TERM", &child.id().to_string()]).status();
            let deadline = Instant::now() + Duration::from_secs(30);
            while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn host(root: &Path, config: &Path, visible: bool) -> Host {
    let mut command = Command::new(env!("CARGO_BIN_EXE_horizon-browser"));
    command.args(["host", "--config"]).arg(config);
    if visible {
        command.arg("--visible");
    }
    command
        .env("HOME", root)
        .env("HORIZON_BROWSER_ROOT", root.join("state"))
        .env_remove("HORIZON_BROWSER_ACTOR")
        .env_remove("HORIZON_BROWSER_HOST_INSTANCE")
        .env("RUST_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map(|child| Host(Some(child)))
        .expect("start the profile host")
}

/// The record that the host writes when it is ready.
fn record(root: &Path, pid: u32) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let path = root.join("state").join("profile-host.json");
        if let Some(record) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(|record| record["pid"] == pid)
        {
            let mode = std::fs::metadata(&path).expect("record").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the record is private");
            return record;
        }
        assert!(Instant::now() < deadline, "the host wrote no record");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn terminate(host: Host) -> (bool, String) {
    let mut child = host.into_child();
    let pid = child.id().to_string();
    Command::new("kill").args(["-TERM", &pid]).status().expect("kill");
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status;
        }
        assert!(Instant::now() < deadline, "the host did not stop after SIGTERM");
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr")
        .read_to_string(&mut stderr)
        .expect("stderr");
    (status.success(), stderr)
}

fn finish(host: Host) -> (Option<i32>, String) {
    let mut child = host.into_child();
    let status = child.wait().expect("wait");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr")
        .read_to_string(&mut stderr)
        .expect("stderr");
    (status.code(), stderr)
}

/// One `horizon-browser mcp --connect` client of an actor and a host instance.
struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Client {
    fn start(root: &Path, actor: &str, instance: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
            .args(["mcp", "--connect"])
            .env("HOME", root)
            .env("HORIZON_BROWSER_ROOT", root.join("state"))
            .env("HORIZON_BROWSER_ACTOR", actor)
            .env("HORIZON_BROWSER_HOST_INSTANCE", instance)
            .env_remove("HORIZON_BROWSER_CONFIG")
            .env("RUST_LOG", "warn")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the MCP client");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut client = Self {
            child,
            stdin,
            stdout,
            next: 1,
        };
        client.call(
            "initialize",
            &json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}),
        );
        client.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        client
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
                "the client closed stdout"
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
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.wait();
    }
}

#[test]
fn a_bad_host_file_and_a_missing_config_are_refused() {
    let root = tempfile::tempdir().expect("root");
    let config = host_file(root.path(), "browser:\n  profiles:\n    Mail: {backend: chromium}\n");
    let (code, stderr) = finish(host(root.path(), &config, false));
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("a profile name is"), "{stderr}");
    let config = host_file(root.path(), HOST_FILE);
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o666)).expect("mode");
    let (code, stderr) = finish(host(root.path(), &config, false));
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("must not be writable"), "{stderr}");
    let missing = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
        .args(["host", "--visible"])
        .output()
        .expect("run");
    assert_eq!(missing.status.code(), Some(2));
}

#[test]
fn one_host_runs_on_a_root_keeps_its_instance_and_keeps_its_profiles() {
    let root = tempfile::tempdir().expect("root");
    let config = host_file(root.path(), HOST_FILE);
    let first = host(root.path(), &config, false);
    let instance = record(root.path(), first.id())["host_instance"]
        .as_str()
        .expect("instance")
        .to_owned();
    let profile = root.path().join("state/browser-profiles/profile-mail");
    assert!(profile.is_dir(), "the host makes the folder of each profile");
    let mode = std::fs::metadata(&profile).expect("profile").permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);

    let (code, stderr) = finish(host(root.path(), &config, false));
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("another profile host runs"), "{stderr}");

    let (success, stderr) = terminate(first);
    assert!(success, "SIGTERM ends the host with 0: {stderr}");
    assert!(profile.is_dir(), "a stop keeps the profile");

    let again = host(root.path(), &config, false);
    let record = record(root.path(), again.id());
    assert_eq!(
        record["host_instance"],
        instance.as_str(),
        "the instance stays over a restart"
    );
    let (success, stderr) = terminate(again);
    assert!(success, "{stderr}");
}

#[test]
fn a_connect_client_must_name_a_profile_and_no_remote_target() {
    let root = tempfile::tempdir().expect("root");
    let config = host_file(root.path(), HOST_FILE);
    let child = host(root.path(), &config, false);
    let instance = record(root.path(), child.id())["host_instance"]
        .as_str()
        .expect("instance")
        .to_owned();
    let mut client = Client::start(root.path(), "horizon:example/ws/agent", &instance);
    let none = client.tool("browser_create", &json!({})).unwrap_err();
    assert!(none.contains("target_required"), "{none}");
    let unknown = client.tool("browser_create", &json!({"target": "nope"})).unwrap_err();
    assert!(unknown.contains("target_unknown"), "{unknown}");
    let device = format!("catalog.browserstack.{}", "a".repeat(64));
    let remote = client.tool("browser_create", &json!({"target": device})).unwrap_err();
    assert!(remote.contains("target_invalid"), "{remote}");
    let usage = client.tool("browser_provider_usage", &json!({})).unwrap_err();
    assert!(usage.contains("provider_usage_unavailable"), "{usage}");
    let listed = client.tool("browser_list", &json!({})).expect("list");
    assert_eq!(listed["panels"], json!([]), "{listed}");
    drop(client);
    let (success, stderr) = terminate(child);
    assert!(success, "{stderr}");
}

/// A loopback page that sets a persistent cookie at `/login` and shows the
/// cookie that it got at `/`.
fn fixture() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let address = format!("http://{}", listener.local_addr().expect("address"));
    std::thread::spawn(move || {
        // One thread for each connection: Chromium can open a connection
        // ahead of time and send nothing on it.
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || answer(&stream));
        }
    });
    address
}

/// Answer one request of the fixture.
fn answer(stream: &TcpStream) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let mut cookie = String::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
            break;
        }
        if let Some(value) = header
            .strip_prefix("Cookie:")
            .or_else(|| header.strip_prefix("cookie:"))
        {
            value.trim().clone_into(&mut cookie);
        }
    }
    let login = line.starts_with("GET /login");
    let body = format!(
        "<!doctype html><title>{}</title><p id=c>{cookie}</p>",
        if login { "in" } else { "page" }
    );
    let set = if login {
        "Set-Cookie: probe=signed-in; Max-Age=86400; Path=/; HttpOnly\r\n"
    } else {
        ""
    };
    let _ = write!(
        &*stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n{set}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// The snapshot of a page once it contains `expected`, or the last one.
fn page_text(client: &mut Client, panel: &str, expected: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snapshot = client
            .tool("browser_snapshot", &json!({"panel_id": panel}))
            .expect("snapshot");
        let text = snapshot.to_string();
        if text.contains(expected) || Instant::now() >= deadline {
            return text;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
#[ignore = "requires a local Chromium browser"]
fn two_actors_share_the_cookies_of_a_profile_see_only_their_own_page_and_keep_them_over_a_restart() {
    let site = fixture();
    let root = tempfile::tempdir().expect("root");
    let config = host_file(root.path(), HOST_FILE);
    let child = host(root.path(), &config, false);
    let instance = record(root.path(), child.id())["host_instance"]
        .as_str()
        .expect("instance")
        .to_owned();
    let mut first = Client::start(root.path(), "horizon:example/one/agent", &instance);
    let mut second = Client::start(root.path(), "horizon:example/two/agent", &instance);
    let created = first
        .tool(
            "browser_create",
            &json!({"target": "mail", "url": format!("{site}/login")}),
        )
        .expect("first create");
    let one = created["panel"]["panel_id"].as_str().expect("panel").to_owned();
    assert!(
        one.starts_with("profile-mail-") && one.len() == "profile-mail-".len() + 12,
        "{one}"
    );
    // The first page has the cookie once it shows the sign-in page.
    let login = format!("{site}/login");
    assert!(
        page_text(&mut first, &one, &login).contains(&login),
        "the first page signs in"
    );
    let created = second
        .tool("browser_create", &json!({"target": "mail", "url": format!("{site}/")}))
        .expect("second create");
    let two = created["panel"]["panel_id"].as_str().expect("panel").to_owned();
    assert!(
        page_text(&mut second, &two, "signed-in").contains("signed-in"),
        "the second page shares the cookie"
    );
    let copy = first
        .tool("browser_duplicate", &json!({"panel_id": one}))
        .expect("duplicate");
    let copy = copy["panel"]["panel_id"].as_str().expect("panel").to_owned();
    assert!(copy.starts_with("profile-mail-") && copy != one, "{copy}");
    assert!(
        page_text(&mut first, &copy, &login).contains(&login),
        "a duplicate opens the URL of its source page"
    );
    let listed = first.tool("browser_list", &json!({})).expect("list").to_string();
    assert!(
        listed.contains(&one) && listed.contains(&copy) && !listed.contains(&two),
        "{listed}"
    );
    let foreign = first.tool("browser_snapshot", &json!({"panel_id": two})).unwrap_err();
    assert!(foreign.contains("outside"), "{foreign}");
    let foreign = first.tool("browser_close", &json!({"panel_id": two})).unwrap_err();
    assert!(foreign.contains("outside"), "{foreign}");
    assert!(
        second
            .tool("browser_list", &json!({}))
            .expect("list")
            .to_string()
            .contains(&two)
    );
    let stranger = Client::start(root.path(), "horizon:example/one/agent", "another-instance")
        .tool("browser_list", &json!({}))
        .expect("list of another instance")
        .to_string();
    assert!(!stranger.contains("profile-mail-"), "{stranger}");
    drop((first, second));
    let (success, stderr) = terminate(child);
    assert!(success, "{stderr}");

    let child = host(root.path(), &config, false);
    record(root.path(), child.id());
    let mut again = Client::start(root.path(), "horizon:example/one/agent", &instance);
    let created = again
        .tool("browser_create", &json!({"target": "mail", "url": format!("{site}/")}))
        .expect("create after the restart");
    let page = created["panel"]["panel_id"].as_str().expect("panel").to_owned();
    assert!(
        page_text(&mut again, &page, "signed-in").contains("signed-in"),
        "the cookie stays over a restart"
    );
    drop(again);
    let (success, stderr) = terminate(child);
    assert!(success, "{stderr}");
}

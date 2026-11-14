//! A mock `DevTools` browser on a loopback port, so a test walks from the
//! options of a create to the commands that the browser of a host receives.
//!
//! The endpoint file of that machine names a port, `ssh` carries that port, and
//! the browser behind it answers `/json/version` and then the `CDP` commands of
//! the session on a WebSocket of that port. Every command stays in the log of the
//! mock, so a test reads the order of the browser-context commands that a private
//! panel sends.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tungstenite::Message;

/// The answer of a mock browser to one command.
type Answer = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

/// The mock browser of one test: the port that its endpoint names and the log of
/// the commands that its session sent.
#[derive(Clone)]
pub(super) struct MockCdpBrowser {
    log: Arc<Mutex<Vec<Value>>>,
}

impl MockCdpBrowser {
    /// Start a mock browser on `port`, with the ordinary answers of a browser of
    /// a browser host.
    ///
    /// The port is the loopback port of the forward that the stand-in `ssh` of a
    /// test carries, so the mock browser answers where the session of the panel
    /// looks for it.
    pub(super) fn start(port: u16) -> Self {
        Self::answering(port, Arc::new(ordinary_answer), None)
    }

    /// Start a mock browser on `port` that answers `answer` instead of the
    /// ordinary answer of one method.
    pub(super) fn answering(port: u16, answer: Answer, replace: Option<&'static str>) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).expect("bind the mock browser");
        let port = listener.local_addr().expect("mock address").port();
        let log: Arc<Mutex<Vec<Value>>> = Arc::default();
        let seen = Arc::clone(&log);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let seen = Arc::clone(&seen);
                let answer = Arc::clone(&answer);
                std::thread::spawn(move || {
                    let _ = serve(stream, port, seen, answer, replace);
                });
            }
        });
        Self { log }
    }

    /// The commands that the session sent so far, oldest first.
    pub(super) fn commands(&self) -> Vec<Value> {
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The methods of the commands that the session sent, oldest first.
    pub(super) fn methods(&self) -> Vec<String> {
        self.commands()
            .iter()
            .filter_map(|command| command["method"].as_str().map(str::to_string))
            .collect()
    }
}

/// Answer one link: the version document on `GET /json/version`, and then the
/// commands of one `CDP` session.
fn serve(
    stream: TcpStream,
    port: u16,
    log: Arc<Mutex<Vec<Value>>>,
    answer: Answer,
    replace: Option<&'static str>,
) -> std::io::Result<()> {
    // The first line of the request decides: a plain `GET` reads the version
    // document of the browser, and an upgrade becomes the command link.
    let mut peeked = [0_u8; 512];
    let read = stream.peek(&mut peeked)?;
    if String::from_utf8_lossy(&peeked[..read]).starts_with("GET /json/version") {
        return answer_version(stream, port);
    }
    let mut socket = tungstenite::accept(stream).expect("the command link");
    while let Ok(Message::Text(text)) = socket.read() {
        let Ok(command) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        log.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(command.clone());
        let result = if replace == command["method"].as_str() {
            answer(&command)
        } else {
            ordinary_answer(&command)
        };
        let reply = json!({ "id": command["id"], "result": result });
        socket
            .send(Message::Text(reply.to_string().into()))
            .expect("answer the command");
    }
    Ok(())
}

/// The ordinary answer of a browser of a browser host.
fn ordinary_answer(command: &Value) -> Value {
    match command["method"].as_str() {
        Some("Target.createBrowserContext") => json!({ "browserContextId": "ctx-private" }),
        Some("Target.createTarget") => json!({ "targetId": "its-page" }),
        Some("Target.attachToTarget") => json!({ "sessionId": "its-session" }),
        Some("Target.closeTarget") => json!({ "success": true }),
        Some("Target.disposeBrowserContext") => json!({}),
        Some("Target.getBrowserContexts") => json!({ "browserContextIds": [] }),
        Some("Target.getTargets") => json!({ "targetInfos": [
            { "targetId": "a-page-of-the-profile", "type": "page", "attached": false }
        ] }),
        _ => json!({}),
    }
}

fn answer_version(mut stream: TcpStream, port: u16) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? <= 2 {
            break;
        }
    }
    let body = format!(r#"{{"webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/browser/mock"}}"#);
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

//! Signing in for one cloud. [`Mode::Ask`] shows a device code for one Authorize
//! click; [`Mode::Automatic`] asks the host to open GitHub's authorize page in a
//! browser that is already signed in, which redirects back to a loopback port here
//! without a click.
use super::{Event, Mode, Runner, Settings};
use base64::Engine as _;
use horizon_cloud::github::{Chain, Client, Error as GitHubError, Poll, Secret};
use sha2::Digest as _;
use std::{
    collections::HashSet,
    io::{Read as _, Write as _},
    net::{TcpListener, TcpStream},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant, SystemTime},
};

/// How long a web sign-in waits for GitHub's redirect.
const WEB_TIMEOUT: Duration = Duration::from_mins(2);
/// How often a wait checks for cancellation and a skip.
const TICK: Duration = Duration::from_millis(200);

/// What the person sees, or what the host must do, for a cloud's GitHub access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// Enter `user_code` at `verification_uri` and click Authorize.
    Device {
        user_code: String,
        verification_uri: String,
        expires_at: SystemTime,
    },
    /// Open `url` in a signed-in browser; GitHub redirects back without a click.
    Web { url: String },
    /// The worker holds access to these repositories as `login`.
    Connected { login: String, repositories: Vec<String> },
    /// The cloud continues without GitHub access, for this reason.
    Ended(String),
}

/// Clouds whose person chose to continue without GitHub access.
static SKIPPED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// Ends the sign-in that waits for `cloud_id`; the cloud continues without GitHub access.
pub fn skip(cloud_id: &str) {
    if let Ok(mut skipped) = SKIPPED.lock() {
        skipped.insert(cloud_id.to_owned());
    }
}

fn skipped(cloud_id: &str) -> bool {
    SKIPPED.lock().is_ok_and(|mut skipped| skipped.remove(cloud_id))
}

/// Why no chain came back.
pub(super) enum Ended {
    /// The cloud continues without GitHub access.
    Reason(String),
    /// The deployment was cancelled or a local file failed.
    Error(super::Error),
}

impl From<super::Error> for Ended {
    fn from(error: super::Error) -> Self {
        Self::Error(error)
    }
}

impl From<std::io::Error> for Ended {
    fn from(error: std::io::Error) -> Self {
        Self::Error(error.into())
    }
}

impl From<GitHubError> for Ended {
    fn from(error: GitHubError) -> Self {
        Self::Reason(error.to_string())
    }
}

pub(super) fn chain(
    settings: &Settings,
    cloud_id: &str,
    client: &Client,
    runner: &Runner<'_>,
) -> std::result::Result<Chain, Ended> {
    // A skip from an earlier attempt does not end this one.
    skipped(cloud_id);
    match settings.mode {
        Mode::Ask => device(settings, cloud_id, client, runner),
        Mode::Automatic => web(settings, cloud_id, client, runner),
    }
}

fn device(
    settings: &Settings,
    cloud_id: &str,
    client: &Client,
    runner: &Runner<'_>,
) -> std::result::Result<Chain, Ended> {
    let mut code = client.start_device(&settings.client_id)?;
    (runner.emit)(Event::GitHub(Prompt::Device {
        user_code: code.user_code.clone(),
        verification_uri: code.verification_uri.clone(),
        expires_at: code.expires_at,
    }));
    loop {
        wait(code.interval, cloud_id, runner)?;
        if SystemTime::now() >= code.expires_at {
            return Err(GitHubError::Expired.into());
        }
        match client.poll_device(&settings.client_id, &mut code)? {
            Poll::Granted(chain) => return Ok(chain),
            Poll::Pending | Poll::SlowDown(_) => {}
        }
    }
}

fn web(settings: &Settings, cloud_id: &str, client: &Client, runner: &Runner<'_>) -> std::result::Result<Chain, Ended> {
    let secret = settings.client_secret()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let redirect = format!("http://127.0.0.1:{}/callback", listener.local_addr()?.port());
    let state = uuid::Uuid::new_v4().simple().to_string();
    let verifier = Secret::new(format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    ));
    let url = authorize_url(&settings.client_id, &redirect, &state, &challenge(&verifier));
    (runner.emit)(Event::GitHub(Prompt::Web { url }));
    let deadline = Instant::now() + WEB_TIMEOUT;
    let code = loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(code) = callback(stream, &state) {
                    break code;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(Ended::Reason(
                        "GitHub did not finish the sign-in in time. Connect GitHub again on the cloud card.".into(),
                    ));
                }
                wait(TICK, cloud_id, runner)?;
            }
            Err(error) => return Err(super::Error::from(error).into()),
        }
    };
    Ok(client.exchange_code(&settings.client_id, &secret, &code, &redirect, &verifier)?)
}

/// Sleeps `duration` in short steps, ending early on cancellation or a skip.
fn wait(duration: Duration, cloud_id: &str, runner: &Runner<'_>) -> std::result::Result<(), Ended> {
    let until = Instant::now() + duration;
    loop {
        runner.cancel.check().map_err(super::Error::from)?;
        if skipped(cloud_id) {
            return Err(Ended::Reason("Skipped: this cloud has no GitHub access.".into()));
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        std::thread::sleep(left.min(TICK));
    }
}

pub(super) fn challenge(verifier: &Secret) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.expose().as_bytes()))
}

pub(super) fn authorize_url(client_id: &str, redirect: &str, state: &str, challenge: &str) -> String {
    let redirect: String = redirect
        .bytes()
        .map(|b| match b {
            b':' => "%3A".to_owned(),
            b'/' => "%2F".to_owned(),
            _ => char::from(b).to_string(),
        })
        .collect();
    format!(
        "https://github.com/login/oauth/authorize?client_id={client_id}&redirect_uri={redirect}&state={state}&code_challenge={challenge}&code_challenge_method=S256"
    )
}

/// Reads one request on the loopback port. Returns the code of a callback whose
/// `state` matches; any other request gets a plain answer and is ignored.
pub(super) fn callback(mut stream: TcpStream, state: &str) -> Option<Secret> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    // Read the whole request head, so closing the connection never resets it.
    let mut buffer = zeroize::Zeroizing::new(vec![0; 4096]);
    let mut read = 0;
    while read < buffer.len() && !buffer[..read].windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut buffer[read..]) {
            Ok(0) | Err(_) => break,
            Ok(count) => read += count,
        }
    }
    let code = std::str::from_utf8(&buffer[..read])
        .ok()
        .and_then(|request| matching_code(request, state));
    let body = if code.is_some() {
        "GitHub is connected for this cloud. You can close this page."
    } else {
        "Horizon did not expect this request."
    };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    code
}

/// The code of a `GET /callback` request whose `state` matches.
fn matching_code(request: &str, state: &str) -> Option<Secret> {
    let target = request.strip_prefix("GET ")?.split(' ').next()?;
    let query = target.strip_prefix("/callback?")?;
    let mut code = None;
    let mut matched = false;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("code", value)) if valid_code(value) => code = Some(Secret::new(value.to_owned())),
            Some(("state", value)) => matched = value == state,
            _ => {}
        }
    }
    code.filter(|_| matched)
}

fn valid_code(code: &str) -> bool {
    (1..=100).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric())
}

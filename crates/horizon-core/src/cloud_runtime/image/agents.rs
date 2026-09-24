//! Newest published agent CLI releases, looked up before an image build so each
//! agent's install layer is keyed on an exact release instead of a cached `latest`.
use super::{Error, Result};
use horizon_cloud::{Agent, Cancellation};
use std::time::Duration;

const REGISTRY: &str = "https://registry.npmjs.org";
const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_DOCUMENT_BYTES: u64 = 1024 * 1024;
const MAX_VERSION_BYTES: usize = 64;

/// Every agent CLI a worker image can install. A repository image may install an
/// agent its profile does not enable, so builds receive a release for each one.
pub const AGENTS: [Agent; 3] = [Agent::Codex, Agent::Claude, Agent::Grok];

/// Where an agent CLI is published and which build argument carries its release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: &'static str,
    pub title: &'static str,
    pub build_argument: &'static str,
    lookup_failed: &'static str,
    invalid_release: &'static str,
}

impl Package {
    #[must_use]
    pub const fn of(agent: Agent) -> Self {
        match agent {
            Agent::Codex => Self {
                name: "@openai/codex",
                title: "Codex",
                build_argument: "HORIZON_CODEX_VERSION",
                lookup_failed: "Could not look up the latest Codex release",
                invalid_release: "The latest Codex release has an unsupported version",
            },
            Agent::Claude => Self {
                name: "@anthropic-ai/claude-code",
                title: "Claude Code",
                build_argument: "HORIZON_CLAUDE_VERSION",
                lookup_failed: "Could not look up the latest Claude Code release",
                invalid_release: "The latest Claude Code release has an unsupported version",
            },
            Agent::Grok => Self {
                name: "@xai-official/grok",
                title: "Grok",
                build_argument: "HORIZON_GROK_VERSION",
                lookup_failed: "Could not look up the latest Grok release",
                invalid_release: "The latest Grok release has an unsupported version",
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    agent: Agent,
    version: String,
}

impl Release {
    /// # Errors
    /// Rejects anything but a plain semantic version, so the value is safe as a
    /// build argument and as an npm version spec.
    pub fn new(agent: Agent, version: impl Into<String>) -> Result<Self> {
        let version = version.into();
        if !valid_version(&version) {
            return Err(Error::Invalid(Package::of(agent).invalid_release));
        }
        Ok(Self { agent, version })
    }

    #[must_use]
    pub const fn agent(&self) -> Agent {
        self.agent
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Releases(Vec<Release>);

impl FromIterator<Release> for Releases {
    fn from_iter<T: IntoIterator<Item = Release>>(releases: T) -> Self {
        Self(releases.into_iter().collect())
    }
}

impl Releases {
    #[must_use]
    pub fn version(&self, agent: Agent) -> Option<&str> {
        self.0
            .iter()
            .find(|release| release.agent == agent)
            .map(|release| release.version.as_str())
    }

    /// `NAME=VERSION` values for `docker build --build-arg`.
    pub fn build_arguments(&self) -> impl Iterator<Item = String> + '_ {
        self.0
            .iter()
            .map(|release| format!("{}={}", Package::of(release.agent).build_argument, release.version))
    }

    #[must_use]
    pub fn summary(&self) -> String {
        self.0
            .iter()
            .map(|release| format!("{} {}", Package::of(release.agent).title, release.version))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Looks up the release that npm currently tags `latest` for every agent CLI.
///
/// # Errors
/// Returns cancellation, or names the agent whose release could not be looked up
/// or is not a plain semantic version. Each lookup is bounded by a timeout.
pub fn latest(cancel: &Cancellation) -> Result<Releases> {
    latest_from(REGISTRY, cancel, TIMEOUT)
}

fn latest_from(registry: &str, cancel: &Cancellation, timeout: Duration) -> Result<Releases> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .max_redirects(0)
        .http_status_as_error(false)
        .build();
    let http = ureq::Agent::new_with_config(config);
    let mut releases = Releases::default();
    for agent in AGENTS {
        cancel.check()?;
        releases.0.push(lookup(&http, registry, agent)?);
    }
    cancel.check()?;
    Ok(releases)
}

#[derive(serde::Deserialize)]
struct Document {
    name: String,
    version: String,
}

fn lookup(http: &ureq::Agent, registry: &str, agent: Agent) -> Result<Release> {
    let package = Package::of(agent);
    let failed = |reason: &dyn std::fmt::Display| {
        tracing::warn!(package = package.name, %reason, "agent release lookup failed");
        Error::Invalid(package.lookup_failed)
    };
    let mut response = http
        .get(format!("{registry}/{}/latest", package.name))
        .header("Accept", "application/json")
        .call()
        .map_err(|error| failed(&error))?;
    if response.status() != 200 {
        return Err(failed(&response.status()));
    }
    let document: Document = response
        .body_mut()
        .with_config()
        .limit(MAX_DOCUMENT_BYTES)
        .read_json()
        .map_err(|error| failed(&error))?;
    if document.name != package.name {
        return Err(Error::Invalid(package.invalid_release));
    }
    Release::new(agent, document.version)
}

/// `MAJOR.MINOR.PATCH` with optional pre-release and build identifiers.
fn valid_version(value: &str) -> bool {
    if value.len() > MAX_VERSION_BYTES {
        return false;
    }
    let (core, suffix) = value.find(['-', '+']).map_or((value, ""), |at| value.split_at(at));
    let numeric = |part: &str| {
        !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) && (part == "0" || !part.starts_with('0'))
    };
    let identifiers = |part: &str| {
        part.split('.')
            .all(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    };
    let (prerelease, build) = suffix
        .split_once('+')
        .map_or((suffix, None), |(pre, build)| (pre, Some(build)));
    let parts = core.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| numeric(part))
        && (prerelease.is_empty() || prerelease.strip_prefix('-').is_some_and(identifiers))
        && build.is_none_or(identifiers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    type Requests = Arc<Mutex<Vec<String>>>;

    /// Answers one connection per response and records each request line.
    fn server(responses: Vec<(u16, String)>) -> (String, Requests, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let requests = Requests::default();
        let observed = requests.clone();
        let task = thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut input = Vec::new();
                let mut buffer = [0; 4096];
                while !input.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).unwrap();
                    assert_ne!(read, 0, "client closed before sending a request");
                    input.extend_from_slice(&buffer[..read]);
                }
                let request = String::from_utf8(input).unwrap();
                observed
                    .lock()
                    .unwrap()
                    .push(request.lines().next().unwrap_or_default().to_owned());
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} Status\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (address, requests, task)
    }

    fn document(agent: Agent, version: &str) -> (u16, String) {
        (
            200,
            serde_json::json!({"name": Package::of(agent).name, "version": version, "dist": {}}).to_string(),
        )
    }

    fn resolve(responses: Vec<(u16, String)>) -> (Result<Releases>, Vec<String>) {
        let (address, requests, task) = server(responses);
        let result = latest_from(&address, &Cancellation::default(), Duration::from_secs(5));
        task.join().unwrap();
        let requests = requests.lock().unwrap().clone();
        (result, requests)
    }

    #[test]
    fn resolves_every_agent_from_its_latest_tag_into_build_arguments() {
        let (result, requests) = resolve(vec![
            document(Agent::Codex, "0.156.1"),
            document(Agent::Claude, "2.1.281"),
            document(Agent::Grok, "1.0.41-beta.2+build.7"),
        ]);
        assert_eq!(
            requests,
            [
                "GET /@openai/codex/latest HTTP/1.1",
                "GET /@anthropic-ai/claude-code/latest HTTP/1.1",
                "GET /@xai-official/grok/latest HTTP/1.1",
            ]
        );
        let releases = result.unwrap();
        assert_eq!(releases.version(Agent::Claude), Some("2.1.281"));
        assert_eq!(
            releases.build_arguments().collect::<Vec<_>>(),
            [
                "HORIZON_CODEX_VERSION=0.156.1",
                "HORIZON_CLAUDE_VERSION=2.1.281",
                "HORIZON_GROK_VERSION=1.0.41-beta.2+build.7",
            ]
        );
        assert_eq!(
            releases.summary(),
            "Codex 0.156.1, Claude Code 2.1.281, Grok 1.0.41-beta.2+build.7"
        );
    }

    #[test]
    fn registry_failures_name_the_agent_and_stop_the_lookup() {
        for (status, body) in [
            (404, r#"{"error":"Not found"}"#.to_owned()),
            (500, String::new()),
            (302, String::new()),
            (200, "not json".to_owned()),
            (200, r#"{"name":"@anthropic-ai/claude-code"}"#.to_owned()),
            (
                200,
                format!(
                    r#"{{"name":"@anthropic-ai/claude-code","version":"{}"}}"#,
                    "1".repeat(2 << 20)
                ),
            ),
        ] {
            let (result, requests) = resolve(vec![document(Agent::Codex, "0.156.1"), (status, body)]);
            assert_eq!(
                result.unwrap_err().to_string(),
                "Could not look up the latest Claude Code release",
                "status {status}"
            );
            assert_eq!(requests.len(), 2, "grok must not be requested after a failure");
        }
    }

    #[test]
    fn unsafe_or_mismatched_releases_are_rejected() {
        for (name, version) in [
            ("@openai/codex", "1.2.3 --build-arg OTHER=value"),
            ("@openai/codex", "$(id)"),
            ("@openai/codex", "latest"),
            ("@openai/other", "1.2.3"),
        ] {
            let body = serde_json::json!({"name": name, "version": version}).to_string();
            let (result, _) = resolve(vec![(200, body)]);
            assert_eq!(
                result.unwrap_err().to_string(),
                "The latest Codex release has an unsupported version"
            );
        }
    }

    #[test]
    fn version_validation_accepts_only_plain_semantic_versions() {
        for version in [
            "0.156.1",
            "2.1.281",
            "10.0.0",
            "1.0.0-alpha.1",
            "1.0.0-rc-1",
            "1.0.0+build.5",
            "1.0.0-0.3.7+x",
        ] {
            assert!(valid_version(version), "{version}");
        }
        let long = format!("1.2.3-{}", "a".repeat(64));
        for version in [
            "",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.02.3",
            "v1.2.3",
            "1.2.3-",
            "1.2.3+",
            "1.2.3-a..b",
            "1.2.3 ",
            "1.2.3\n",
            "1.2.3;id",
            "1.2.3-a_b",
            "1.2.3-ä",
            "^1.2.3",
            long.as_str(),
        ] {
            assert!(!valid_version(version), "{version:?}");
        }
    }

    #[test]
    #[ignore = "queries the public npm registry"]
    fn public_registry_tags_a_plain_release_for_every_agent() {
        let releases = latest(&Cancellation::default()).unwrap();
        for agent in AGENTS {
            assert!(releases.version(agent).is_some(), "{agent:?}");
        }
        println!("{}", releases.summary());
    }

    #[test]
    fn cancellation_prevents_lookups_and_unresponsive_registries_time_out() {
        let cancel = Cancellation::default();
        cancel.cancel();
        let error = latest_from("http://127.0.0.1:9", &cancel, Duration::from_secs(5)).unwrap_err();
        assert!(matches!(error, Error::Provider(horizon_cloud::CloudError::Cancelled)));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let started = std::time::Instant::now();
        let error = latest_from(&address, &Cancellation::default(), Duration::from_millis(300)).unwrap_err();
        assert_eq!(error.to_string(), "Could not look up the latest Codex release");
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(listener);
    }
}

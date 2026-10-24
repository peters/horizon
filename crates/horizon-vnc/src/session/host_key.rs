//! The SSH host key of a tunnelled viewer: what ssh refused, and what a probe
//! observed. Horizon never trusts a key for a caller, so this evidence exists
//! for a person to compare with the machine out of band.
use std::{path::Path, process::Stdio, time::Duration};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use sha2::{Digest as _, Sha256};
use tokio::process::Command;

/// How long a refusal waits for `ssh-keyscan` to answer.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long `ssh-keyscan` tries one address.
const PROBE_CONNECT_TIMEOUT_SECS: &str = "3";
/// The algorithm whose key a person is shown first. A server offers one key
/// per algorithm it has, so the list picks the strongest one it offered.
const KEY_TYPE_PREFERENCE: [&str; 5] = [
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "ssh-rsa",
];

/// The SSH host a tunnel names, the program that reaches it and the program's
/// own arguments, so a probe can read the same SSH configuration.
#[derive(Clone, Debug)]
pub(super) struct TunnelHost {
    /// The ssh program; its directory also holds `ssh-keyscan`.
    pub program: String,
    pub host: String,
    /// The port the route names. None leaves the port to the SSH configuration.
    pub port: Option<u16>,
    /// The tunnel's arguments, which start with `-W <remote endpoint>`.
    pub args: Vec<String>,
}

/// Why ssh refused the host key of the tunnel's host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKeyRefusal {
    /// ssh has no `known_hosts` entry for this host, so it refuses to connect.
    NotTrusted,
    /// `known_hosts` holds a different key for this host: a verification failure.
    Changed,
}

/// A host-key refusal with the evidence a person needs to decide what to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyFailure {
    pub refusal: HostKeyRefusal,
    /// The SSH host as the viewer's route named it.
    pub host: String,
    /// The SSH port when the route named one; otherwise ssh decided it.
    pub port: Option<u16>,
    /// The key the host presented to an unauthenticated probe, when one answered.
    /// It is the key that the tunnel's own ssh saw, because the probe repeats
    /// that ssh's configuration.
    pub observed: Option<ObservedHostKey>,
    /// Why no direct probe can see the key that ssh saw, when a skip applies.
    pub unobserved: Option<UnobservedReason>,
    /// The host and port the probe used, when the SSH configuration resolved
    /// them to something other than the route's host and port.
    pub resolved: Option<String>,
}

/// Why Horizon did not observe the key that ssh refused. A probe that reads no
/// SSH configuration could reach another server or compare another key, so it
/// does not run for these cases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnobservedReason {
    /// The configuration sends this connection through a jump host or through
    /// a proxy command.
    Proxy,
    /// The configuration names the host key with `HostKeyAlias`, so a direct
    /// probe would compare a different key.
    HostKeyAlias,
}

/// A host key that a probe saw. Observing a key is not trust: an impostor
/// answers with its own key, so a person compares the fingerprint out of band.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedHostKey {
    /// The SSH key algorithm, for example `ssh-ed25519`.
    pub key_type: String,
    /// The fingerprint in the form `ssh-keygen -lf` prints.
    pub sha256: String,
}

impl HostKeyFailure {
    /// The refusal that ssh's lines name, if they name one. A bare
    /// `Host key verification failed.` stays untyped: it can also mean an
    /// entry that a person changed or revoked, which is not a host that
    /// waits to be trusted.
    fn refusal(tail: &str) -> Option<HostKeyRefusal> {
        if tail.contains("REMOTE HOST IDENTIFICATION HAS CHANGED!") {
            Some(HostKeyRefusal::Changed)
        } else if tail.contains("host key is known for") {
            Some(HostKeyRefusal::NotTrusted)
        } else {
            None
        }
    }

    /// The refusal for this host, observing the presented key only when the
    /// host is simply not trusted yet. A changed key is a verification
    /// failure, and a second contact would not decide anything.
    pub(super) async fn detect(tail: &str, host: &TunnelHost) -> Option<Self> {
        let refusal = Self::refusal(tail)?;
        let observation = match refusal {
            HostKeyRefusal::NotTrusted => observe_refused_host(host).await,
            HostKeyRefusal::Changed => Observation::default(),
        };
        Some(Self {
            refusal,
            host: host.host.clone(),
            port: host.port,
            observed: observation.observed,
            unobserved: observation.unobserved,
            resolved: observation.resolved,
        })
    }

    /// The host as ssh names it, with the port when the route named one.
    #[must_use]
    pub fn label(&self) -> String {
        host_label(&self.host, self.port)
    }

    /// What a person can do next, in the terms of the refusal. Horizon has no
    /// operation that trusts a host and accepts no key from a caller.
    #[must_use]
    pub fn advice(&self) -> String {
        let label = self.label();
        if self.refusal == HostKeyRefusal::Changed {
            return format!(
                "known_hosts holds a different key for {label}; a changed host key is a verification failure \
                 that a person checks with the machine out of band, and Horizon never accepts a key from a caller"
            );
        }
        match (&self.observed, self.unobserved) {
            (Some(observed), _) => {
                let at = self
                    .resolved
                    .as_deref()
                    .map_or_else(String::new, |resolved| format!(" at {resolved}"));
                format!(
                    "{label} is not in known_hosts; the host{at} presented {} {}; a person compares that \
                     fingerprint with the machine out of band, adds the key with standard ssh tooling, then reconnects",
                    observed.key_type, observed.sha256
                )
            }
            (None, Some(reason)) => format!(
                "{label} is not in known_hosts; Horizon did not observe its key because {}; a person reads \
                 the host key from the machine out of band, adds it with standard ssh tooling, then reconnects",
                reason.describe()
            ),
            (None, None) => format!(
                "{label} is not in known_hosts and Horizon could not observe its key; a person reads the \
                 host key from the machine out of band, adds it with standard ssh tooling, then reconnects"
            ),
        }
    }
}

impl UnobservedReason {
    /// The reason as a reader sees it in the refusal.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Proxy => {
                "the SSH configuration sends this connection through a proxy jump or a proxy command, so a \
                 direct probe would reach a different server"
            }
            Self::HostKeyAlias => {
                "the SSH configuration names the host key with HostKeyAlias, so a direct probe would \
                 compare a different key"
            }
        }
    }
}

/// What a probe saw, where it looked, and why it did not look.
#[derive(Default)]
struct Observation {
    observed: Option<ObservedHostKey>,
    unobserved: Option<UnobservedReason>,
    resolved: Option<String>,
}

/// Observe the key that the tunnel's own ssh saw. The tunnel reads this
/// machine's SSH configuration, so a probe that reads none of it could show the
/// key of another server: resolve the route with `ssh -G` first, and probe only
/// when the resolved connection is the same one that ssh refused.
async fn observe_refused_host(host: &TunnelHost) -> Observation {
    let Some(config) = effective_config(host).await else {
        return Observation::default();
    };
    let (hostname, port) = match config.probe_target() {
        ProbeTarget::Direct(hostname, port) => (hostname, port),
        ProbeTarget::Skipped(reason) => {
            return Observation {
                unobserved: Some(reason),
                ..Observation::default()
            };
        }
        ProbeTarget::Unknown => return Observation::default(),
    };
    let resolved = host_label(&hostname, port);
    let observed = observe(&host.program, &hostname, port).await;
    Observation {
        resolved: (resolved != host_label(&host.host, host.port)).then_some(resolved),
        observed,
        unobserved: None,
    }
}

/// The settings of the tunnel's own SSH configuration that decide where ssh
/// connects and which key it compares.
#[derive(Default)]
struct EffectiveConfig {
    hostname: Option<String>,
    port: Option<u16>,
    proxyjump: Option<String>,
    proxycommand: Option<String>,
    hostkeyalias: Option<String>,
}

impl EffectiveConfig {
    fn parse(output: &str) -> Self {
        let mut config = Self::default();
        for line in output.lines() {
            let Some((keyword, value)) = line.split_once(' ') else {
                continue;
            };
            let value = value.trim().to_owned();
            match keyword {
                "hostname" => config.hostname = Some(value),
                "port" => config.port = value.parse().ok(),
                "proxyjump" => config.proxyjump = Some(value),
                "proxycommand" => config.proxycommand = Some(value),
                "hostkeyalias" => config.hostkeyalias = Some(value),
                _ => {}
            }
        }
        config
    }

    /// Where a direct probe reaches the server that ssh refused, or why it
    /// does not.
    fn probe_target(&self) -> ProbeTarget {
        if is_set(self.proxyjump.as_ref()) || is_set(self.proxycommand.as_ref()) {
            return ProbeTarget::Skipped(UnobservedReason::Proxy);
        }
        if is_set(self.hostkeyalias.as_ref()) {
            return ProbeTarget::Skipped(UnobservedReason::HostKeyAlias);
        }
        match &self.hostname {
            Some(hostname) if !hostname.is_empty() => ProbeTarget::Direct(hostname.clone(), self.port),
            _ => ProbeTarget::Unknown,
        }
    }
}

/// Where the probe looks, or why it does not.
enum ProbeTarget {
    /// The resolved endpoint, as `ssh -G` answers it for the tunnel.
    Direct(String, Option<u16>),
    /// A configuration that sends the connection elsewhere, which the refusal
    /// explains to a reader.
    Skipped(UnobservedReason),
    /// A configuration that Horizon cannot repeat, with no reason to name.
    Unknown,
}

fn is_set(option: Option<&String>) -> bool {
    option.is_some_and(|value| !value.is_empty() && value != "none")
}

/// Ask the tunnel's ssh what its configuration resolves the route to. It
/// connects to nothing and writes nothing.
async fn effective_config(host: &TunnelHost) -> Option<EffectiveConfig> {
    let args = resolve_args(&host.args)?;
    let mut command = Command::new(&host.program);
    command.arg("-G");
    command.args(&args);
    let output = run(command).await?;
    output
        .status
        .success()
        .then(|| EffectiveConfig::parse(&String::from_utf8_lossy(&output.stdout)))
}

/// The arguments of `ssh -G`: the tunnel's own options and target, without its
/// `-W <remote endpoint>`. A tunnel that does not start that way is not
/// resolved, so a probe never guesses where the tunnel connects.
fn resolve_args(tunnel_args: &[String]) -> Option<Vec<String>> {
    match tunnel_args {
        [flag, _endpoint, rest @ ..] if flag == "-W" && !rest.is_empty() => Some(rest.to_vec()),
        _ => None,
    }
}

/// Run one probe process with the bounded timeout and no input.
async fn run(mut command: Command) -> Option<std::process::Output> {
    tokio::time::timeout(
        PROBE_TIMEOUT,
        command.stdin(Stdio::null()).stderr(Stdio::null()).output(),
    )
    .await
    .ok()?
    .ok()
}

/// `host` or `[host]:port`, as ssh names a host.
fn host_label(host: &str, port: Option<u16>) -> String {
    port.map_or_else(|| host.to_owned(), |port| format!("[{host}]:{port}"))
}

/// Observe the host key that a host presents, through `ssh-keyscan`. It writes
/// nothing to `known_hosts` and trusts nothing.
async fn observe(program: &str, host: &str, port: Option<u16>) -> Option<ObservedHostKey> {
    let keyscan = Path::new(program).with_file_name("ssh-keyscan");
    let mut command = Command::new(keyscan);
    command.arg("-T").arg(PROBE_CONNECT_TIMEOUT_SECS);
    if let Some(port) = port {
        command.arg("-p").arg(port.to_string());
    }
    command.arg(host);
    let output = run(command).await?;
    parse_keyscan(&output.stdout)
}

/// The strongest key in `ssh-keyscan` output, with the fingerprint a person
/// compares. Each key line is `host key-type base64-key`; comments start with
/// `#` and carry the banner.
fn parse_keyscan(output: &[u8]) -> Option<ObservedHostKey> {
    let text = String::from_utf8_lossy(output);
    let offered = || {
        text.lines().filter(|line| !line.starts_with('#')).filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _host = fields.next()?;
            let key_type = fields.next()?;
            let key = fields.next()?;
            Some((key_type, key))
        })
    };
    KEY_TYPE_PREFERENCE.iter().find_map(|preferred| {
        offered()
            .filter(|(key_type, _)| key_type == preferred)
            .find_map(|(key_type, key)| {
                Some(ObservedHostKey {
                    key_type: key_type.to_owned(),
                    sha256: sha256_fingerprint(key)?,
                })
            })
    })
}

/// `SHA256:` and the unpadded base64 of the key blob's SHA-256, which is the
/// form `ssh-keygen -lf` prints. The blob is the decoded key line, not its text.
fn sha256_fingerprint(key: &str) -> Option<String> {
    let blob = STANDARD.decode(key).ok()?;
    Some(format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(blob))))
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _, path::PathBuf};

    use super::*;

    /// A synthetic host key blob, with the fingerprint that `ssh-keygen -lf`
    /// printed for it. No key pair and no server are needed.
    const ED25519_KEY: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIC3gDh/ATgAW1C2pZQvrgzjlkoHoJffEJkJ3dvWAkIdK";
    const ED25519_SHA256: &str = "SHA256:ZCf2G4oj6NBiKuZG8dQbSNWd7S0CvUOHYAOPJf5m6o8";
    const RSA_KEY: &str = "AAAAB3NzaC1yc2EAAAADAQABAAABAQDcHE4OpYC6aSxV4xST6/PtOp2qGbFrLUiZvgBzpPJPuxl/6+qWn1tcwx2mQs88XGPPE3LghL2j7V3KA2MzCh5iCa6QUb2grsboyuvCPWvFZMBMtoFeYDHPiu7qqolxdL1yHK2HUi/w/3X4gt5GKCNDE4E20rB4YsKmmfLcQUOJtUUcfTw5bQUTx47yjcl5Myun+O8vadOCypOhNWmYC8EVz9zU2p+Q5VM5F9sP+K02Gvodzvcq449tpqGjKQLPJvwitsG8J5CKy1JQ7RvxwtmgHgdCUUnJcRYQfUWH13UZeQPXuUYIJrm+0hzDuuhjoScimTkcxuZ1i9xL7CgMromn";
    const RSA_SHA256: &str = "SHA256:SVT1peLsTv8svV25JxZ2/z00+Vv5CY8/azW/6Cgf0Ok";
    /// The argument that a stand-in program answers without running its body.
    const READY_ARG: &str = "--horizon-vnc-test-ready";
    const READY: &str = "if [ \"$1\" = \"--horizon-vnc-test-ready\" ]; then exit 0; fi";

    struct Scratch(PathBuf);

    impl Scratch {
        /// An empty directory holding the programs a probe runs.
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "horizon-vnc-host-key-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("scratch directory");
            Self(path)
        }

        /// A stand-in program that runs `body` as a shell script.
        fn script(&self, name: &str, body: &str) {
            let path = self.0.join(name);
            fs::write(&path, format!("#!/bin/sh\n{READY}\n{body}\n")).expect("program");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
            self.make_runnable(name);
        }

        /// Wait until the kernel lets this program run. Another test thread can
        /// fork while the file is still open for writing, and an exec then fails
        /// with `Text file busy`; retry until that window closes.
        fn make_runnable(&self, name: &str) {
            let path = self.0.join(name);
            for _ in 0..200 {
                let status = std::process::Command::new(&path)
                    .arg(READY_ARG)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                if status.is_ok_and(|status| status.success()) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("{name} never became runnable");
        }

        /// A stand-in `ssh` that records the arguments of each call and answers
        /// `ssh -G` with `settings`, one line for each item.
        fn ssh_config(&self, settings: &[&str]) {
            let record = self.path_string("ssh-args");
            let lines = settings
                .iter()
                .map(|line| format!("'{line}'"))
                .collect::<Vec<_>>()
                .join(" ");
            self.script(
                "ssh",
                &format!("printf '%s\\n' \"$@\" > {record}\nprintf '%s\\n' {lines}"),
            );
        }

        /// A stand-in `ssh-keyscan` that records its arguments and prints `line`.
        fn keyscan(&self, line: &str) {
            let record = self.path_string("keyscan-args");
            self.script(
                "ssh-keyscan",
                &format!("printf '%s\\n' \"$@\" > {record}\nprintf '%s\\n' '{line}'"),
            );
        }

        fn path_string(&self, name: &str) -> String {
            self.0.join(name).to_string_lossy().into_owned()
        }

        /// The arguments that a stand-in program recorded, when it ran.
        fn recorded(&self, name: &str) -> Option<String> {
            fs::read_to_string(self.0.join(name)).ok()
        }

        fn ssh(&self) -> String {
            self.path_string("ssh")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_a_named_host_key_case_is_typed() {
        let unknown = "No ED25519 host key is known for [lab]:22 and you have requested strict checking.; \
            Host key verification failed.";
        assert_eq!(HostKeyFailure::refusal(unknown), Some(HostKeyRefusal::NotTrusted));

        let changed = "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!; Host key verification failed.";
        assert_eq!(HostKeyFailure::refusal(changed), Some(HostKeyRefusal::Changed));

        assert_eq!(
            HostKeyFailure::refusal("Host key verification failed."),
            None,
            "a bare verification failure can also be an entry a person revoked"
        );
        assert_eq!(
            HostKeyFailure::refusal("channel 0: open failed: connect failed: Connection refused"),
            None
        );
    }

    #[test]
    fn the_strongest_offered_key_is_the_one_a_person_compares() {
        let output = format!(
            "# lab:22 SSH-2.0-OpenSSH_9.6\r\n\
             # lab:22 SSH-2.0-OpenSSH_9.6\r\n\
             lab ssh-rsa {RSA_KEY}\r\n\
             lab ssh-ed25519 {ED25519_KEY}\r\n"
        );
        let observed = parse_keyscan(output.as_bytes()).expect("an offered key");
        assert_eq!(observed.key_type, "ssh-ed25519");
        assert_eq!(observed.sha256, ED25519_SHA256, "the form ssh-keygen -lf prints");

        let rsa_only = format!("[lab]:2222 ssh-rsa {RSA_KEY}\r\n");
        let observed = parse_keyscan(rsa_only.as_bytes()).expect("an offered key");
        assert_eq!(observed.key_type, "ssh-rsa");
        assert_eq!(observed.sha256, RSA_SHA256);
    }

    #[test]
    fn comments_and_silence_observe_no_key() {
        assert_eq!(parse_keyscan(b"# lab:22 SSH-2.0-OpenSSH_9.6\r\n"), None);
        assert_eq!(parse_keyscan(b""), None);
        assert_eq!(parse_keyscan(b"lab ssh-ed25519\r\n"), None, "a line without a key");
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    /// A tunnel host shaped as `Session` builds it: `-W` first, then the SSH
    /// options and the target.
    fn tunnel_host(program: &str, host: &str, port: Option<u16>, options: &[&str]) -> TunnelHost {
        let mut args = vec!["-W".to_string(), "127.0.0.1:5900".to_string()];
        args.extend(options.iter().map(|option| (*option).to_string()));
        args.push(host.to_string());
        TunnelHost {
            program: program.into(),
            host: host.into(),
            port,
            args,
        }
    }

    #[test]
    fn an_untrusted_host_reports_the_key_it_presented() {
        runtime().block_on(async {
            let scratch = Scratch::new();
            scratch.ssh_config(&["hostname lab", "port 2222"]);
            scratch.keyscan(&format!("[lab]:2222 ssh-ed25519 {ED25519_KEY}"));
            let host = tunnel_host(&scratch.ssh(), "lab", Some(2222), &[]);
            let failure = HostKeyFailure::detect("No ED25519 host key is known for [lab]:2222.", &host)
                .await
                .expect("a typed refusal");
            assert_eq!(failure.refusal, HostKeyRefusal::NotTrusted);
            assert_eq!(failure.label(), "[lab]:2222");
            assert_eq!(failure.unobserved, None);
            assert_eq!(
                failure.resolved, None,
                "the resolution repeats the route, so nothing is added to the advice"
            );
            assert_eq!(
                failure.observed,
                Some(ObservedHostKey {
                    key_type: "ssh-ed25519".into(),
                    sha256: ED25519_SHA256.into()
                })
            );
            let advice = failure.advice();
            assert!(advice.contains("[lab]:2222"), "{advice}");
            assert!(advice.contains(ED25519_SHA256), "{advice}");
            assert!(advice.contains("out of band"), "{advice}");
        });
    }

    #[test]
    fn the_ssh_configuration_decides_the_port_the_probe_uses() {
        runtime().block_on(async {
            let scratch = Scratch::new();
            scratch.ssh_config(&["hostname lab", "port 2222"]);
            scratch.keyscan(&format!("[lab]:2222 ssh-ed25519 {ED25519_KEY}"));
            let host = tunnel_host(&scratch.ssh(), "lab", None, &["-o", "BatchMode=yes"]);
            let failure = HostKeyFailure::detect("No ED25519 host key is known for [lab]:2222.", &host)
                .await
                .expect("a typed refusal");
            assert_eq!(failure.port, None, "the route named no port");
            assert_eq!(
                failure.resolved.as_deref(),
                Some("[lab]:2222"),
                "the advice names where the key was seen"
            );
            assert_eq!(
                failure.observed.as_ref().map(|observed| observed.sha256.as_str()),
                Some(ED25519_SHA256)
            );
            let ssh_args = scratch.recorded("ssh-args").expect("ssh resolved the configuration");
            assert!(
                ssh_args.starts_with("-G\n"),
                "the probe asks ssh for its settings: {ssh_args}"
            );
            assert!(
                ssh_args.contains("BatchMode=yes"),
                "the probe repeats the tunnel's options: {ssh_args}"
            );
            assert!(
                !ssh_args.contains("-W"),
                "the probe never repeats the forward: {ssh_args}"
            );
            let probe = scratch.recorded("keyscan-args").expect("the probe ran");
            assert!(probe.contains("-p\n2222\n"), "the configured port is probed: {probe}");
            assert!(probe.ends_with("lab\n"), "the resolved host is probed: {probe}");
            assert!(failure.advice().contains("at [lab]:2222"), "{}", failure.advice());
        });
    }

    #[test]
    fn the_ssh_configuration_decides_the_host_name_the_probe_uses() {
        runtime().block_on(async {
            let scratch = Scratch::new();
            scratch.ssh_config(&["hostname lab.internal.example", "port 22"]);
            scratch.keyscan(&format!("[lab.internal.example]:22 ssh-ed25519 {ED25519_KEY}"));
            let host = tunnel_host(&scratch.ssh(), "lab", None, &[]);
            let failure = HostKeyFailure::detect("No ED25519 host key is known for [lab]:22.", &host)
                .await
                .expect("a typed refusal");
            assert_eq!(failure.resolved.as_deref(), Some("[lab.internal.example]:22"));
            assert_eq!(
                failure.observed.as_ref().map(|observed| observed.sha256.as_str()),
                Some(ED25519_SHA256)
            );
            let probe = scratch.recorded("keyscan-args").expect("the probe ran");
            assert!(
                probe.ends_with("lab.internal.example\n"),
                "the alias is resolved before the probe: {probe}"
            );
            assert!(
                failure.advice().contains("at [lab.internal.example]:22"),
                "{}",
                failure.advice()
            );
        });
    }

    #[test]
    fn a_proxy_jump_or_proxy_command_skips_the_probe() {
        runtime().block_on(async {
            for settings in [
                ["hostname lab", "port 22", "proxyjump bastion"].as_slice(),
                ["hostname lab", "port 22", "proxycommand nc %h %p"].as_slice(),
            ] {
                let scratch = Scratch::new();
                scratch.ssh_config(settings);
                scratch.keyscan(&format!("lab ssh-ed25519 {ED25519_KEY}"));
                let host = tunnel_host(&scratch.ssh(), "lab", None, &[]);
                let failure = HostKeyFailure::detect("No ED25519 host key is known for lab.", &host)
                    .await
                    .expect("a typed refusal");
                assert_eq!(failure.observed, None);
                assert_eq!(failure.unobserved, Some(UnobservedReason::Proxy));
                assert_eq!(
                    scratch.recorded("keyscan-args"),
                    None,
                    "a direct probe would reach another server: {settings:?}"
                );
                let advice = failure.advice();
                assert!(advice.contains("proxy jump or a proxy command"), "{advice}");
                assert!(advice.contains("standard ssh tooling"), "{advice}");
            }
        });
    }

    #[test]
    fn a_host_key_alias_skips_the_probe() {
        runtime().block_on(async {
            let scratch = Scratch::new();
            scratch.ssh_config(&["hostname lab", "port 22", "hostkeyalias pinned"]);
            scratch.keyscan(&format!("lab ssh-ed25519 {ED25519_KEY}"));
            let host = tunnel_host(&scratch.ssh(), "lab", None, &[]);
            let failure = HostKeyFailure::detect("No ED25519 host key is known for lab.", &host)
                .await
                .expect("a typed refusal");
            assert_eq!(failure.observed, None);
            assert_eq!(failure.unobserved, Some(UnobservedReason::HostKeyAlias));
            assert_eq!(
                scratch.recorded("keyscan-args"),
                None,
                "a direct probe would compare a different key"
            );
            let advice = failure.advice();
            assert!(advice.contains("HostKeyAlias"), "{advice}");
            assert!(advice.contains("reads the host key from the machine"), "{advice}");
        });
    }

    #[test]
    fn a_tunnel_argument_list_that_does_not_start_with_a_forward_is_never_resolved() {
        let forwarded = ["-W".to_string(), "127.0.0.1:5900".to_string(), "lab".to_string()];
        assert_eq!(resolve_args(&forwarded), Some(vec!["lab".to_string()]));
        assert_eq!(
            resolve_args(&["-o".to_string(), "BatchMode=yes".to_string(), "lab".to_string()]),
            None,
            "an argument list of another shape must not guess where the tunnel connects"
        );
        assert_eq!(resolve_args(&["-W".to_string(), "127.0.0.1:5900".to_string()]), None);
    }

    #[test]
    fn a_probe_that_cannot_run_still_names_the_untrusted_host() {
        runtime().block_on(async {
            let host = tunnel_host("/nonexistent/horizon-vnc-test/ssh", "192.0.2.10", None, &[]);
            let failure = HostKeyFailure::detect("No ED25519 host key is known for 192.0.2.10.", &host)
                .await
                .expect("a typed refusal");
            assert_eq!(failure.observed, None);
            assert_eq!(failure.unobserved, None);
            assert_eq!(failure.resolved, None);
            assert_eq!(failure.label(), "192.0.2.10");
            assert!(failure.advice().contains("could not observe its key"));
        });
    }

    #[test]
    fn a_changed_key_is_never_probed_again() {
        runtime().block_on(async {
            let host = tunnel_host("/nonexistent/horizon-vnc-test/ssh", "lab", Some(22), &[]);
            let failure = HostKeyFailure::detect("WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!", &host)
                .await
                .expect("a typed refusal");
            assert_eq!(failure.refusal, HostKeyRefusal::Changed);
            assert_eq!(failure.observed, None);
            assert_eq!(failure.unobserved, None);
            let advice = failure.advice();
            assert!(advice.contains("verification failure"), "{advice}");
            assert!(
                !advice.contains("adds the key"),
                "a changed key must not invite an accept: {advice}"
            );
        });
    }

    #[test]
    fn an_unrelated_failure_stays_untyped() {
        runtime().block_on(async {
            let host = tunnel_host("/nonexistent/horizon-vnc-test/ssh", "lab", None, &[]);
            assert_eq!(
                HostKeyFailure::detect("ssh: connect to host lab port 22: Connection refused", &host).await,
                None
            );
        });
    }
}

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

/// The SSH host a tunnel names, and the program that reaches it.
#[derive(Clone, Debug)]
pub(super) struct TunnelHost {
    /// The ssh program; its directory also holds `ssh-keyscan`.
    pub program: String,
    pub host: String,
    /// The port the route names. None leaves the port to the SSH configuration.
    pub port: Option<u16>,
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
    pub observed: Option<ObservedHostKey>,
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
        let observed = match refusal {
            HostKeyRefusal::NotTrusted => observe(&host.program, &host.host, host.port).await,
            HostKeyRefusal::Changed => None,
        };
        Some(Self {
            refusal,
            host: host.host.clone(),
            port: host.port,
            observed,
        })
    }

    /// The host as ssh names it, with the port when the route named one.
    #[must_use]
    pub fn label(&self) -> String {
        self.port
            .map_or_else(|| self.host.clone(), |port| format!("[{}]:{port}", self.host))
    }

    /// What a person can do next, in the terms of the refusal. Horizon has no
    /// operation that trusts a host and accepts no key from a caller.
    #[must_use]
    pub fn advice(&self) -> String {
        match (self.refusal, &self.observed) {
            (HostKeyRefusal::NotTrusted, Some(observed)) => format!(
                "{} is not in known_hosts; the host presented {} {}; a person verifies that fingerprint \
                 with the machine out of band, adds the key with standard ssh tooling, then reconnects",
                self.label(),
                observed.key_type,
                observed.sha256
            ),
            (HostKeyRefusal::NotTrusted, None) => format!(
                "{} is not in known_hosts and Horizon could not observe its key; a person adds the host \
                 key with standard ssh tooling, then reconnects",
                self.label()
            ),
            (HostKeyRefusal::Changed, _) => format!(
                "known_hosts holds a different key for {}; a changed host key is a verification failure \
                 that a person checks with the machine out of band, and Horizon never accepts a key from a caller",
                self.label()
            ),
        }
    }
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
    let output = tokio::time::timeout(
        PROBE_TIMEOUT,
        command.arg(host).stdin(Stdio::null()).stderr(Stdio::null()).output(),
    )
    .await
    .ok()?
    .ok()?;
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

        /// A stand-in program that prints `text` on standard output.
        fn program(&self, name: &str, text: &str) {
            let path = self.0.join(name);
            fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' '{text}'\n")).expect("program");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
        }

        fn ssh(&self) -> String {
            self.0.join("ssh").to_string_lossy().into_owned()
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

    #[test]
    fn an_untrusted_host_reports_the_key_it_presented() {
        runtime().block_on(async {
            let scratch = Scratch::new();
            scratch.program("ssh", "");
            scratch.program("ssh-keyscan", &format!("[lab]:2222 ssh-ed25519 {ED25519_KEY}"));
            let host = TunnelHost {
                program: scratch.ssh(),
                host: "lab".into(),
                port: Some(2222),
            };
            let failure = HostKeyFailure::detect("No ED25519 host key is known for [lab]:2222.", &host)
                .await
                .expect("a typed refusal");
            assert_eq!(failure.refusal, HostKeyRefusal::NotTrusted);
            assert_eq!(failure.label(), "[lab]:2222");
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
    fn a_probe_that_cannot_run_still_names_the_untrusted_host() {
        runtime().block_on(async {
            let host = TunnelHost {
                program: "/nonexistent/horizon-vnc-test/ssh".into(),
                host: "192.0.2.10".into(),
                port: None,
            };
            let failure = HostKeyFailure::detect("No ED25519 host key is known for 192.0.2.10.", &host)
                .await
                .expect("a typed refusal");
            assert_eq!(failure.observed, None);
            assert_eq!(failure.label(), "192.0.2.10");
            assert!(failure.advice().contains("could not observe its key"));
        });
    }

    #[test]
    fn a_changed_key_is_never_probed_again() {
        runtime().block_on(async {
            let host = TunnelHost {
                program: "/nonexistent/horizon-vnc-test/ssh".into(),
                host: "lab".into(),
                port: Some(22),
            };
            let failure = HostKeyFailure::detect("WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!", &host)
                .await
                .expect("a typed refusal");
            assert_eq!(failure.refusal, HostKeyRefusal::Changed);
            assert_eq!(failure.observed, None);
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
            let host = TunnelHost {
                program: "/nonexistent/horizon-vnc-test/ssh".into(),
                host: "lab".into(),
                port: None,
            };
            assert_eq!(
                HostKeyFailure::detect("ssh: connect to host lab port 22: Connection refused", &host).await,
                None
            );
        });
    }
}

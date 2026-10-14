//! The read-only Check of a saved Docker host in Cloud settings. Each row goes out as
//! soon as it is known. It runs `docker version`, `docker info` and lists Horizon's own
//! containers; it starts, pulls and installs nothing.
use super::{Check, CheckState};
use crate::cloud_runtime::Cancellation;
use horizon_cloud::{
    CloudError,
    docker_host::{self, Access, Capacity, Docker, Engine, Failure, Host},
};
use std::{path::Path, time::Duration};

/// Long enough for SSH to sign in and docker to answer; a Tailscale SSH check mode that
/// waits for a browser runs out of it.
const ANSWER: Duration = Duration::from_secs(20);

/// Checks `host` with this computer's private Docker configuration under `root`, and
/// returns its capacity when the engine answered.
pub fn check(host: &Host, root: &Path, cancel: &Cancellation, row: &mut dyn FnMut(Check)) -> Option<Capacity> {
    let engine = Engine {
        host,
        local_config: root.join("docker-host-check"),
        cancel,
    };
    rows(host, &engine, row)
}

fn rows(host: &Host, docker: &impl Docker, row: &mut dyn FnMut(Check)) -> Option<Capacity> {
    if let Err(error) = host.validate() {
        send(row, "Host", CheckState::Blocked, error.to_string(), None);
        return None;
    }
    if !reached(host, docker, row) {
        return None;
    }
    let capacity = match docker_host::capacity(docker) {
        Ok(capacity) => capacity,
        Err(error) => {
            send(row, "Docker engine", CheckState::Blocked, error.to_string(), None);
            return None;
        }
    };
    let engine = format!("Docker {} runs Linux containers", capacity.version);
    send(row, "Docker engine", CheckState::Ready, engine, None);
    if capacity.native() {
        let detail = format!("{}: the worker image runs natively", capacity.architecture);
        send(row, "Architecture", CheckState::Ready, detail, None);
    } else {
        let detail = format!("{}: the worker image has no build for it", capacity.architecture);
        let remedy = Some("Use an x86_64 or arm64 host.");
        send(row, "Architecture", CheckState::Blocked, detail, remedy);
    }
    let roomy = capacity.fits(2, 4);
    let state = if roomy { CheckState::Ready } else { CheckState::Warning };
    let remedy = (!roomy).then_some("Stop a cloud on this host, or give Docker more CPUs and memory.");
    send(row, "Free capacity", state, capacity.free(), remedy);
    Some(capacity)
}

fn send(row: &mut dyn FnMut(Check), name: &str, state: CheckState, detail: String, remedy: Option<&str>) {
    row(Check {
        name: name.into(),
        state,
        detail,
        remedy: remedy.map(str::to_owned),
    });
}

/// Signs in to the host and runs docker there as its user. Returns whether docker
/// answered.
fn reached(host: &Host, docker: &impl Docker, row: &mut dyn FnMut(Check)) -> bool {
    let address = host.address.as_deref().unwrap_or("this computer");
    let (way, remedy) = match host.access {
        Access::ThisPc => ("", ""),
        Access::Ssh => (
            "SSH",
            "Check that ssh to this address signs in without a password prompt, with your keys or agent and a known host key.",
        ),
        Access::TailscaleSsh => (
            "Tailscale SSH",
            "Check that the host's Tailscale SSH rule uses action accept (check mode waits for a browser) and allows this user.",
        ),
    };
    let remote = host.access != Access::ThisPc;
    let version = ["version", "--format", "{{.Server.Version}}"].map(str::to_owned);
    let answer = docker.docker(&version, ANSWER);
    if let Err(Failure::Error(error)) = &answer {
        let detail = match (remote, error) {
            (true, CloudError::Transport) => format!("Horizon could not sign in to {address} over {way}"),
            (true, error) => format!("{address} did not answer over {way}: {error}"),
            (false, error) => error.to_string(),
        };
        let (name, remedy) = if remote {
            ("Connection", Some(remedy))
        } else {
            ("Docker access", None)
        };
        send(row, name, CheckState::Blocked, detail, remedy);
        return false;
    }
    if remote {
        let detail = format!("Signed in to {address} over {way}");
        send(row, "Connection", CheckState::Ready, detail, None);
    }
    if let Err(Failure::Refused(text)) = &answer {
        let (detail, remedy) = refusal(text);
        send(row, "Docker access", CheckState::Blocked, detail, Some(remedy));
        return false;
    }
    let detail = "docker answers for this user".into();
    send(row, "Docker access", CheckState::Ready, detail, None);
    true
}

/// What a docker that ran and refused needs, from what it wrote.
fn refusal(text: &str) -> (String, &'static str) {
    let lower = text.to_ascii_lowercase();
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("docker refused");
    if lower.contains("not found") || lower.contains("no such file") {
        (
            "docker was not found on the host".into(),
            "Set the docker path, such as /usr/local/bin/docker.",
        )
    } else if lower.contains("permission denied") {
        (
            "This user cannot use the Docker engine".into(),
            "Add the user to the docker group, then sign in again.",
        )
    } else if lower.contains("cannot connect to the docker daemon") {
        ("The Docker engine is not running".into(), "Start Docker on the host.")
    } else {
        (
            first.chars().take(200).collect(),
            "Check the docker path and Docker on the host.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Answers each docker call with the next canned result.
    struct Answers(RefCell<Vec<Result<String, Failure>>>);

    impl Docker for Answers {
        fn docker(&self, _args: &[String], _timeout: Duration) -> Result<String, Failure> {
            self.0.borrow_mut().remove(0)
        }
    }

    fn checked(access: Access, answers: Vec<Result<String, Failure>>) -> (Option<Capacity>, Vec<Check>) {
        let host = Host {
            name: "build-box".into(),
            access,
            address: (access != Access::ThisPc).then(|| "deploy@build-box".into()),
            docker: None,
        };
        let mut rows_sent = Vec::new();
        let capacity = rows(&host, &Answers(RefCell::new(answers)), &mut |check| {
            rows_sent.push(check);
        });
        (capacity, rows_sent)
    }

    fn states(checks: &[Check]) -> Vec<(&str, CheckState)> {
        checks.iter().map(|check| (check.name.as_str(), check.state)).collect()
    }

    #[test]
    fn a_reachable_host_reports_its_engine_architecture_and_free_capacity() {
        let info =
            r#"{"OSType":"linux","NCPU":16,"MemTotal":68719476736,"ServerVersion":"28.5.1","Architecture":"aarch64"}"#;
        let (capacity, rows) = checked(
            Access::Ssh,
            vec![Ok("28.5.1".into()), Ok(info.into()), Ok(String::new())],
        );
        assert_eq!(capacity.unwrap().cpus, 16);
        assert_eq!(
            states(&rows),
            [
                ("Connection", CheckState::Ready),
                ("Docker access", CheckState::Ready),
                ("Docker engine", CheckState::Ready),
                ("Architecture", CheckState::Ready),
                ("Free capacity", CheckState::Ready),
            ]
        );
        assert_eq!(rows[4].detail, "16.0 of 16 CPUs and 64.0 of 64.0 GB free");
    }

    #[test]
    fn each_failure_stops_the_check_with_its_remedy() {
        let (capacity, rows) = checked(Access::TailscaleSsh, vec![Err(Failure::Error(CloudError::Transport))]);
        assert!(capacity.is_none());
        assert_eq!(states(&rows), [("Connection", CheckState::Blocked)]);
        assert!(rows[0].remedy.as_deref().unwrap().contains("accept"));
        let denied = "permission denied while trying to connect to the Docker daemon socket";
        let (_, rows) = checked(Access::Ssh, vec![Err(Failure::Refused(denied.into()))]);
        assert_eq!(
            states(&rows),
            [
                ("Connection", CheckState::Ready),
                ("Docker access", CheckState::Blocked)
            ]
        );
        assert!(rows[1].remedy.as_deref().unwrap().contains("docker group"));
        let (_, rows) = checked(
            Access::ThisPc,
            vec![Err(Failure::Refused("sh: docker: not found".into()))],
        );
        assert_eq!(rows[0].detail, "docker was not found on the host");
        let old = r#"{"OSType":"linux","NCPU":2,"MemTotal":1073741824,"ServerVersion":"27.5.0"}"#;
        let (_, rows) = checked(Access::ThisPc, vec![Ok("27.5.0".into()), Ok(old.into())]);
        assert_eq!(rows.last().unwrap().state, CheckState::Blocked);
        assert!(
            rows.last().unwrap().detail.contains("28"),
            "{}",
            rows.last().unwrap().detail
        );
    }
}

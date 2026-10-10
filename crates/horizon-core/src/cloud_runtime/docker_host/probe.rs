//! Missing host requirements block admission; image validation follows in deployment.
use super::{Binding, Error, Result, SshAuthentication, Transport};
use crate::cloud_runtime::{Cancellation, command::Runner};
use horizon_cloud::Profile;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    Ready,
    Warning,
    Blocked,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub state: CheckState,
    pub detail: String,
    pub remedy: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Probe {
    pub host_id: String,
    pub observed_at: u64,
    pub checks: Vec<Check>,
    pub cpu: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub architecture: Option<String>,
    pub free_bytes: Option<u64>,
}

impl Probe {
    #[must_use]
    pub fn ready(&self) -> bool {
        !self.checks.is_empty()
            && self
                .checks
                .iter()
                .all(|check| matches!(check.state, CheckState::Ready | CheckState::Warning))
    }

    fn check(&mut self, name: &str, state: CheckState, detail: impl Into<String>, remedy: Option<&str>) {
        self.checks.push(Check {
            name: name.into(),
            state,
            detail: detail.into(),
            remedy: remedy.map(str::to_owned),
        });
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Info {
    #[serde(rename = "NCPU")]
    cpu: u64,
    mem_total: u64,
    architecture: String,
    #[serde(rename = "OSType")]
    os_type: String,
    docker_root_dir: String,
    server_version: String,
    #[serde(default)]
    operating_system: String,
    #[serde(default)]
    kernel_version: String,
}

/// # Errors
/// Invalid bindings and cancellation fail; missing host prerequisites become card rows.
pub fn probe(host: &Binding, profile: Option<&Profile>, cancel: &Cancellation) -> Result<Probe> {
    host.validate()?;
    cancel.check()?;
    let runner = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let transport = Transport(host);
    let mut result = Probe {
        host_id: host.id.clone(),
        observed_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        checks: Vec::new(),
        cpu: None,
        memory_bytes: None,
        architecture: None,
        free_bytes: None,
    };
    if cfg!(windows) {
        result.check(
            "Controller platform",
            CheckState::Blocked,
            "Docker host deployments need a Linux or macOS Horizon controller",
            Some("Use Horizon on Linux or macOS. Windows cloud records do not yet support durable directory updates."),
        );
        return Ok(result);
    }
    if !ssh_check(host, &runner, &mut result)? {
        return Ok(result);
    }
    let Ok(current) = transport.run(&runner, &["context", "show"], Duration::from_secs(8)) else {
        cancel.check()?;
        result.check(
            "Docker engine",
            CheckState::Blocked,
            "Docker CLI or selected context is unavailable",
            Some("Install Docker and select a local engine context."),
        );
        return Ok(result);
    };
    let context = current.trim();
    let endpoint = transport.run(
        &runner,
        &["context", "inspect", context, "--format", "{{.Endpoints.docker.Host}}"],
        Duration::from_secs(8),
    );
    if endpoint.is_err() {
        cancel.check()?;
    }
    let local = endpoint
        .as_ref()
        .is_ok_and(|value| value.trim().starts_with("unix://") || value.trim().starts_with("npipe://"));
    result.check(
        "Engine location",
        if local { CheckState::Ready } else { CheckState::Blocked },
        if local {
            "Docker context selects an engine on the chosen host"
        } else {
            "Docker context is missing or forwards to another host"
        },
        (!local)
            .then_some("Select a local engine context on this host. Register forwarded engines as their own SSH host."),
    );
    if !local {
        return Ok(result);
    }
    let info = transport
        .run(&runner, &["info", "--format", "{{json .}}"], Duration::from_secs(10))
        .and_then(|output| serde_json::from_str::<Info>(&output).map_err(|_| Error::Json));
    let Ok(info) = info else {
        cancel.check()?;
        result.check(
            "Docker engine",
            CheckState::Blocked,
            "Docker is missing, stopped, inaccessible or did not answer",
            Some("Install Docker, start its engine, and give the selected account access. Check the Docker context."),
        );
        return Ok(result);
    };
    let mut filesystem = filesystem_state(&info);
    if filesystem == CheckState::Ready {
        filesystem = if let Ok(host) = transport.host_kernel(&runner) {
            host_filesystem_state(&info, &host)
        } else {
            cancel.check()?;
            CheckState::Unknown
        };
    }
    engine_checks(&mut result, &info, &transport, &runner, profile, filesystem)?;
    profile_checks(&mut result, profile);
    Ok(result)
}

fn profile_checks(result: &mut Probe, profile: Option<&Profile>) {
    if let Some(profile) = profile {
        let known = profile.build.is_none()
            && crate::cloud_runtime::repository::launch::quick_start::trusted_contract(
                &profile.image,
                &profile.capabilities,
            )
            .is_some();
        result.check("Worker image", if known { CheckState::Ready } else { CheckState::Warning },
            if known { "Pinned worker image has a published contract" } else { "Worker image needs registry and worker-contract validation before allocation" },
            (!known).then_some("Configure image pull access and a local Docker builder for contract validation. Arbitrary images need Horizon's worker services."));
    }
    if profile.is_some_and(|profile| {
        profile.gpu || profile.capabilities.browserstack.is_some() || profile.idle_stop_minutes.is_some()
    }) {
        result.check(
            "Profile capabilities",
            CheckState::Blocked,
            "This provider supports CPU workers without hosted devices or idle stop",
            Some("Select a CPU profile without hosted devices or idle stop."),
        );
    }
}

fn ssh_check(host: &Binding, runner: &Runner<'_>, result: &mut Probe) -> Result<bool> {
    if let Some(ssh) = &host.ssh {
        let tailscale = ssh.authentication == SshAuthentication::Tailscale;
        if tailscale {
            if let Some(missing) = super::tailscale::missing(ssh, runner)? {
                result.check(
                    "Tailscale SSH",
                    CheckState::Blocked,
                    missing.detail,
                    Some(missing.remedy),
                );
                return Ok(false);
            }
        } else {
            let usable = ssh.identity_file.is_file() && ssh.known_hosts.is_file();
            result.check(
                "SSH identity",
                if usable { CheckState::Ready } else { CheckState::Blocked },
                if usable {
                    "Private key and pinned host-key file are available"
                } else {
                    "SSH identity or trusted host-key file is missing"
                },
                (!usable).then_some("Select an SSH key and trust this host's key before deployment."),
            );
            if !usable {
                return Ok(false);
            }
        }
        let mut connection = Transport(host).ssh()?;
        connection.arg("true");
        if runner
            .run_parsed("SSH probe", &mut connection, Duration::from_secs(8))
            .is_err()
        {
            runner.cancel.check()?;
            result.check(
                "SSH connection",
                CheckState::Blocked,
                if tailscale { "Tailscale SSH access was denied, needs reauthentication, or the host did not answer" } else { "SSH authentication, host trust or connectivity failed" },
                Some(if tailscale { "Check Tailscale connectivity and SSH policy for this Linux user. Complete any Tailscale reauthentication before checking again." } else { "Check the address, user, key, trusted host key and network route." }),
            );
            return Ok(false);
        }
        result.check(
            "SSH connection",
            CheckState::Ready,
            if tailscale {
                "Tailscale identity and tailnet-managed host trust accepted"
            } else {
                "Authenticated connection to the trusted host"
            },
            None,
        );
    }
    Ok(true)
}
fn gib(bytes: u64) -> String {
    let tenth = bytes / 107_374_182;
    format!("{}.{}", tenth / 10, tenth % 10)
}
fn engine_checks(
    result: &mut Probe,
    info: &Info,
    transport: &Transport<'_>,
    runner: &Runner<'_>,
    profile: Option<&Profile>,
    filesystem: CheckState,
) -> Result<()> {
    result.cpu = Some(info.cpu);
    result.memory_bytes = Some(info.mem_total);
    result.architecture = Some(info.architecture.clone());
    result.check("Docker engine", CheckState::Ready, "Docker engine answered", None);
    let safe_ports = info
        .server_version
        .split('.')
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .is_some_and(|major| major >= 28);
    result.check(
        "Private SSH port",
        if safe_ports {
            CheckState::Ready
        } else {
            CheckState::Blocked
        },
        format!("Docker {}", info.server_version),
        (!safe_ports).then_some("Use Docker Engine 28 or later so localhost-published ports are private."),
    );
    result.check(
        "Container platform",
        if info.os_type == "linux" {
            CheckState::Ready
        } else {
            CheckState::Blocked
        },
        format!("{} / {}", info.os_type, info.architecture),
        (info.os_type != "linux").then_some("Select a Linux container engine."),
    );
    result.check(
        "Engine filesystem",
        filesystem,
        match filesystem {
            CheckState::Ready => "Engine reports the chosen Linux host's kernel version",
            CheckState::Blocked => "Engine storage belongs to another operating system or virtual machine",
            _ => "Engine host filesystem could not be confirmed",
        },
        (filesystem != CheckState::Ready)
            .then_some("Use a native Linux Docker engine whose storage is visible to the selected account."),
    );
    if filesystem != CheckState::Ready {
        return Ok(());
    }
    let native = matches!(info.architecture.as_str(), "amd64" | "x86_64");
    result.check(
        "Worker architecture",
        if native {
            CheckState::Ready
        } else if transport.0.allow_emulation {
            CheckState::Warning
        } else {
            CheckState::Blocked
        },
        if native {
            "x86 worker image runs natively"
        } else if transport.0.allow_emulation {
            "x86 emulation was explicitly enabled; performance is reduced"
        } else {
            "This engine needs emulation for Horizon's x86 worker image"
        },
        (!native && !transport.0.allow_emulation).then_some("Use an x86 host or explicitly enable x86 emulation."),
    );
    let fits = profile.is_none_or(|profile| {
        info.cpu >= u64::from(profile.cpu) && info.mem_total >= u64::from(profile.memory_gb) * 1_073_741_824
    });
    result.check(
        "Compute capacity",
        if fits { CheckState::Ready } else { CheckState::Blocked },
        format!("{} CPUs, {} GiB assigned to Docker", info.cpu, gib(info.mem_total)),
        (!fits).then_some("Choose a smaller profile or increase the Docker engine's CPU and memory allocation."),
    );
    if let Ok(free) = transport.disk_free(runner, &info.docker_root_dir) {
        result.free_bytes = Some(free);
        let enough = profile.is_none_or(|profile| {
            free >= (u64::from(profile.storage.volume_gb) + u64::from(profile.storage.container_gb)) * 1_000_000_000
        });
        result.check(
            "Workspace storage",
            if enough { CheckState::Ready } else { CheckState::Blocked },
            format!("{} GiB free; workspace space is shared and not reserved", gib(free)),
            (!enough).then_some("Free space or choose smaller storage requirements."),
        );
    } else {
        runner.cancel.check()?;
        result.check(
            "Workspace storage",
            CheckState::Unknown,
            "Docker workspace free space could not be measured",
            Some("Give the selected account access to the engine's storage filesystem and check again."),
        );
    }
    Ok(())
}

fn filesystem_state(info: &Info) -> CheckState {
    if info.operating_system.is_empty() {
        CheckState::Unknown
    } else if info.operating_system.to_ascii_lowercase().contains("docker desktop")
        || info.kernel_version.to_ascii_lowercase().contains("linuxkit")
    {
        CheckState::Blocked
    } else {
        CheckState::Ready
    }
}

fn host_filesystem_state(info: &Info, host: &str) -> CheckState {
    let Some((system, kernel)) = host.trim().split_once(' ') else {
        return CheckState::Unknown;
    };
    if info.kernel_version.is_empty() {
        CheckState::Unknown
    } else if system != "Linux" || kernel != info.kernel_version {
        CheckState::Blocked
    } else {
        CheckState::Ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(root: &std::path::Path) -> Info {
        Info {
            cpu: 2,
            mem_total: 4 * 1_073_741_824,
            architecture: "amd64".into(),
            os_type: "linux".into(),
            docker_root_dir: root.to_string_lossy().into_owned(),
            server_version: "28.0.0".into(),
            operating_system: "Ubuntu".into(),
            kernel_version: "6.8.0".into(),
        }
    }

    #[test]
    fn vm_backed_and_unconfirmed_hosts_cannot_measure_local_engine_storage() {
        let mut info = engine(std::path::Path::new("/unused"));
        info.kernel_version = "6.8.0".into();
        assert_eq!(host_filesystem_state(&info, "Linux 6.8.0\n"), CheckState::Ready);
        for host in ["Darwin 24.0.0", "Linux 6.14.0"] {
            assert_eq!(host_filesystem_state(&info, host), CheckState::Blocked);
        }
        assert_eq!(host_filesystem_state(&info, ""), CheckState::Unknown);
        info.kernel_version.clear();
        assert_eq!(host_filesystem_state(&info, "Linux 6.8.0"), CheckState::Unknown);
    }

    #[test]
    fn desktop_and_unreported_engine_platforms_cannot_pass_storage_admission() {
        let mut info = engine(std::path::Path::new("/unused"));
        assert_eq!(filesystem_state(&info), CheckState::Ready);
        info.operating_system = "Docker Desktop".into();
        assert_eq!(filesystem_state(&info), CheckState::Blocked);
        info.operating_system = "Alpine Linux".into();
        info.kernel_version = "6.10.14-linuxkit".into();
        assert_eq!(filesystem_state(&info), CheckState::Blocked);
        info.operating_system.clear();
        assert_eq!(filesystem_state(&info), CheckState::Unknown);
    }

    #[test]
    fn a_build_from_the_trusted_base_still_needs_worker_validation() {
        let mut profile = crate::cloud_runtime::repository::launch::quick_start::builtin()
            .unwrap()
            .profiles
            .into_values()
            .next()
            .unwrap();
        let mut report = Probe {
            host_id: "fixture".into(),
            observed_at: 0,
            checks: Vec::new(),
            cpu: None,
            memory_bytes: None,
            architecture: None,
            free_bytes: None,
        };
        profile_checks(&mut report, Some(&profile));
        assert_eq!(report.checks[0].state, CheckState::Ready);
        report.checks.clear();
        profile.build = Some(
            serde_json::from_value(serde_json::json!({
                "context":".", "dockerfile":"Dockerfile"
            }))
            .unwrap(),
        );
        profile_checks(&mut report, Some(&profile));
        assert_eq!(report.checks[0].name, "Worker image");
        assert_eq!(report.checks[0].state, CheckState::Warning);
    }

    // Native engine admission measures storage with the Unix df tool.
    #[cfg(unix)]
    #[test]
    fn native_engine_admission_enforces_version_architecture_and_capacity() {
        let root = tempfile::tempdir().unwrap();
        let host = Binding {
            id: "fixture".into(),
            name: "Fixture".into(),
            ssh: None,
            context: None,
            allow_emulation: false,
        };
        let cancel = Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        let mut profile: Profile = serde_json::from_value(serde_json::json!({
            "provider":"runpod", "image":"fixture", "cpu":1, "memory_gb":1,
            "storage":{"container_gb":0,"volume_gb":0}
        }))
        .unwrap();
        let check = |info: &Info, profile: &Profile| {
            let mut report = Probe {
                host_id: host.id.clone(),
                observed_at: 0,
                checks: Vec::new(),
                cpu: None,
                memory_bytes: None,
                architecture: None,
                free_bytes: None,
            };
            engine_checks(
                &mut report,
                info,
                &Transport(&host),
                &runner,
                Some(profile),
                CheckState::Ready,
            )
            .unwrap();
            report
        };
        let mut info = engine(root.path());
        let ready = check(&info, &profile);
        assert!(ready.ready());
        assert!(ready.free_bytes.is_some());
        profile.cpu = 3;
        profile.memory_gb = 5;
        info.server_version = "27.5.0".into();
        info.architecture = "aarch64".into();
        let blocked = check(&info, &profile);
        for name in ["Private SSH port", "Worker architecture", "Compute capacity"] {
            assert!(
                blocked
                    .checks
                    .iter()
                    .any(|row| row.name == name && row.state == CheckState::Blocked)
            );
        }
        assert!(!blocked.ready());
    }

    // Native Docker engine free space uses the Unix df tool.
    #[cfg(unix)]
    #[test]
    fn an_unmeasurable_engine_directory_keeps_the_probe_unready() {
        let root = tempfile::tempdir().unwrap();
        let host = Binding {
            id: "fixture".into(),
            name: "Fixture".into(),
            ssh: None,
            context: None,
            allow_emulation: false,
        };
        let cancel = Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        let mut report: Probe = serde_json::from_value(serde_json::json!({
            "host_id":"fixture", "observed_at":0, "checks":[],
            "cpu":null,"memory_bytes":null,"architecture":null,"free_bytes":null
        }))
        .unwrap();
        engine_checks(
            &mut report,
            &engine(&root.path().join("missing")),
            &Transport(&host),
            &runner,
            None,
            CheckState::Ready,
        )
        .unwrap();
        assert!(!report.ready());
        assert!(report.free_bytes.is_none());
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.name == "Workspace storage" && check.state == CheckState::Unknown)
        );
    }
}

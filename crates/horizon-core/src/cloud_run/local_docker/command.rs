use super::{
    COMMAND_TIMEOUT, DockerContainer, DockerCreateRequest, DockerPortBinding, DockerResult, DockerTransport,
    HOST_KEY_PATH, SSH_PUBLIC_KEY_ENV, TERMINATE_ENV, invalid_response,
};
use crate::cloud_run::local_docker::LocalDockerError;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io::Read,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(20);
const RESPONSE_LIMIT_BYTES: usize = 1024 * 1024;

pub(super) struct DockerCli {
    executable: OsString,
    docker_host: OsString,
}

impl DockerCli {
    pub(super) fn new(docker_host: &str) -> Self {
        Self {
            executable: OsString::from("docker"),
            docker_host: OsString::from(docker_host),
        }
    }
    fn output<I, S>(&self, args: I, operation: &'static str, timeout: Duration) -> DockerResult<CapturedOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut child = Command::new(&self.executable)
            .arg("--host")
            .arg(&self.docker_host)
            .args(args)
            .env("DOCKER_CLIENT_TIMEOUT", timeout.as_secs().saturating_add(1).to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    LocalDockerError::DockerUnavailable
                } else {
                    LocalDockerError::CommandFailed { operation }
                }
            })?;
        capture_output(&mut child, operation, timeout)
    }
}

impl DockerTransport for DockerCli {
    fn inspect(&self, reference: &str) -> DockerResult<Option<DockerContainer>> {
        self.inspect_with_timeout(reference, COMMAND_TIMEOUT)
    }
    fn inspect_with_timeout(&self, reference: &str, timeout: Duration) -> DockerResult<Option<DockerContainer>> {
        let args = ["container", "inspect", "--", reference];
        let output = self.output(args, "container inspection", timeout)?;
        if !output.status.success() {
            return if object_missing(&output.stderr) {
                Ok(None)
            } else {
                Err(LocalDockerError::CommandFailed {
                    operation: "container inspection",
                })
            };
        }
        parse_inspection(output.stdout_text("container inspection")?).map(Some)
    }
    fn create(&self, request: &DockerCreateRequest) -> Result<String, LocalDockerError> {
        let mut args = vec![
            OsString::from("run"),
            OsString::from("--detach"),
            OsString::from("--pull=never"),
            OsString::from("--restart=no"),
            OsString::from("--name"),
            OsString::from(&request.name),
            OsString::from("--publish"),
            OsString::from("127.0.0.1::22"),
            OsString::from("--env"),
            OsString::from(format!("{SSH_PUBLIC_KEY_ENV}={}", request.ssh_public_key)),
            OsString::from("--env"),
            OsString::from(format!("{TERMINATE_ENV}={}", request.terminate_after)),
        ];
        for (key, value) in &request.labels {
            args.push(OsString::from("--label"));
            args.push(OsString::from(format!("{key}={value}")));
        }
        args.push(OsString::from(&request.image));
        let output = self.output(args, "container creation", COMMAND_TIMEOUT)?;
        if !output.status.success() {
            return Err(LocalDockerError::CommandFailed {
                operation: "container creation",
            });
        }
        output.stdout_text("container creation").map(str::to_string)
    }
    fn read_host_key(&self, resource_id: &str) -> Result<Option<String>, LocalDockerError> {
        let output = self.output(
            ["exec", "--", resource_id, "cat", HOST_KEY_PATH],
            "SSH host-key inspection",
            COMMAND_TIMEOUT,
        )?;
        if !output.status.success() {
            return host_key_unavailable(&output.stderr)
                .then_some(None)
                .ok_or(LocalDockerError::CommandFailed {
                    operation: "SSH host-key inspection",
                });
        }
        Ok(Some(output.stdout_text("SSH host-key inspection")?.to_string()))
    }
    fn delete(&self, resource_id: &str) -> Result<bool, LocalDockerError> {
        let args = ["container", "rm", "--force", "--", resource_id];
        let output = self.output(args, "container deletion", COMMAND_TIMEOUT)?;
        if output.status.success() {
            return Ok(true);
        }
        if object_missing(&output.stderr) {
            Ok(false)
        } else {
            Err(LocalDockerError::CommandFailed {
                operation: "container deletion",
            })
        }
    }
}

struct CapturedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl CapturedOutput {
    fn stdout_text(&self, operation: &'static str) -> Result<&str, LocalDockerError> {
        let value = std::str::from_utf8(&self.stdout)
            .map_err(|_| LocalDockerError::InvalidResponse { operation })?
            .trim();
        if value.is_empty() {
            Err(LocalDockerError::InvalidResponse { operation })
        } else {
            Ok(value)
        }
    }
}
fn capture_output(child: &mut Child, operation: &'static str, timeout: Duration) -> DockerResult<CapturedOutput> {
    let stdout = child
        .stdout
        .take()
        .ok_or(LocalDockerError::InvalidResponse { operation })?;
    let stderr = child
        .stderr
        .take()
        .ok_or(LocalDockerError::InvalidResponse { operation })?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout));
    let stderr_reader = thread::spawn(move || read_bounded(stderr));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|_| LocalDockerError::CommandFailed { operation })?
        {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(LocalDockerError::CommandTimedOut { operation });
        }
        thread::sleep(COMMAND_POLL_INTERVAL.min(timeout.saturating_sub(started.elapsed())));
    };
    let (stdout, stdout_oversized) = stdout_reader
        .join()
        .map_err(|_| LocalDockerError::InvalidResponse { operation })?
        .map_err(|_| LocalDockerError::InvalidResponse { operation })?;
    let (stderr, stderr_oversized) = stderr_reader
        .join()
        .map_err(|_| LocalDockerError::InvalidResponse { operation })?
        .map_err(|_| LocalDockerError::InvalidResponse { operation })?;
    if stdout_oversized || stderr_oversized {
        return Err(LocalDockerError::InvalidResponse { operation });
    }
    Ok(CapturedOutput { status, stdout, stderr })
}
fn read_bounded(mut reader: impl Read) -> std::io::Result<(Vec<u8>, bool)> {
    let mut retained = Vec::new();
    let mut oversized = false;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = RESPONSE_LIMIT_BYTES.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..count.min(remaining)]);
        oversized |= count > remaining;
    }
    Ok((retained, oversized))
}

fn object_missing(stderr: &[u8]) -> bool {
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    stderr.contains("no such object") || stderr.contains("no such container")
}

fn host_key_unavailable(stderr: &[u8]) -> bool {
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    object_missing(stderr.as_bytes())
        || stderr.contains("is not running")
        || (stderr.contains(HOST_KEY_PATH) && stderr.contains("no such file or directory"))
}

fn parse_inspection(value: &str) -> Result<DockerContainer, LocalDockerError> {
    let [record] = serde_json::from_str::<[Inspection; 1]>(value).map_err(|_| invalid_inspection())?;
    let name = record.name.strip_prefix('/').unwrap_or(&record.name).to_string();
    let ssh_bindings = record
        .network_settings
        .ports
        .and_then(|ports| ports.ssh)
        .unwrap_or_default()
        .into_iter()
        .map(|binding| {
            let port = binding.host_port.parse::<u16>().map_err(|_| invalid_inspection())?;
            Ok(DockerPortBinding {
                host: binding.host_ip,
                port,
            })
        })
        .collect::<Result<Vec<_>, LocalDockerError>>()?;
    Ok(DockerContainer {
        id: record.id,
        name,
        image: record.config.image,
        labels: record.config.labels.unwrap_or_default(),
        environment: record.config.env.unwrap_or_default(),
        restart_policy: record.host_config.restart_policy.name,
        running: record.state.running,
        state: record.state.status,
        exit_code: record.state.exit_code,
        ssh_bindings,
    })
}

fn invalid_inspection() -> LocalDockerError {
    invalid_response("container inspection")
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Inspection {
    id: String,
    name: String,
    config: InspectionConfig,
    host_config: InspectionHostConfig,
    state: InspectionState,
    network_settings: InspectionNetworkSettings,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectionConfig {
    image: String,
    labels: Option<BTreeMap<String, String>>,
    env: Option<Vec<String>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectionHostConfig {
    restart_policy: InspectionRestartPolicy,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectionRestartPolicy {
    name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectionState {
    running: bool,
    status: String,
    exit_code: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectionNetworkSettings {
    ports: Option<InspectionPorts>,
}
#[derive(Deserialize)]
struct InspectionPorts {
    #[serde(rename = "22/tcp", default)]
    ssh: Option<Vec<InspectionBinding>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectionBinding {
    host_ip: String,
    host_port: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_decodes_one_loopback_binding() {
        let value = r#"[{"Id":"id","Name":"/horizon-local-worker","Config":{"Image":"image","Labels":null,"Env":null},"HostConfig":{"RestartPolicy":{"Name":"no"}},"State":{"Running":true,"Status":"running","ExitCode":0},"NetworkSettings":{"Ports":{"22/tcp":[{"HostIp":"127.0.0.1","HostPort":"49152"}]}}}]"#;
        let parsed = parse_inspection(value).expect("Docker inspection");
        assert_eq!(parsed.ssh_bindings.len(), 1);
        assert_eq!(parsed.ssh_bindings[0].host, "127.0.0.1");
        assert_eq!(parsed.ssh_bindings[0].port, 49_152);
        assert!(parse_inspection("[]").is_err());
    }

    #[test]
    fn missing_host_key_does_not_hide_a_missing_daemon_socket() {
        let missing_key = format!("cat: {HOST_KEY_PATH}: No such file or directory");
        assert!(host_key_unavailable(missing_key.as_bytes()));
        assert!(!host_key_unavailable(
            b"dial unix /tmp/docker.sock: connect: no such file or directory"
        ));
    }
}

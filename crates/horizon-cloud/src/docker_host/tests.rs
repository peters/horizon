use super::{worker::first_port, *};
use crate::{CreateState, WorkerSpec, WorkerStatus};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeMap, time::Duration};

/// A simulated Docker engine with just enough of the CLI for the worker lifecycle.
#[derive(Default)]
struct Fake {
    containers: RefCell<BTreeMap<String, Value>>,
    /// Volume names and their owner labels.
    volumes: RefCell<BTreeMap<String, Option<String>>>,
    /// Loopback ports that another program holds.
    blocked: RefCell<Vec<u16>>,
    /// Commands, by their first two words, that run out of time.
    stalled: Vec<&'static str>,
    calls: RefCell<Vec<(String, Duration)>>,
    cpus: u64,
    memory_gb: u64,
}

fn flag<'a>(args: &'a [String], name: &str) -> &'a str {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map_or("", |pair| pair[1].as_str())
}

fn container(id: &str, name: &str, owner: Option<&str>, port: &str, running: bool, image: &str, env: &[&str]) -> Value {
    json!({
        "Id": id, "Name": format!("/{name}"),
        "Config": {"Labels": owner.map(|owner| json!({OWNER_LABEL: owner})), "Image": image, "Env": env},
        "HostConfig": {"NanoCpus": 2_000_000_000_u64, "Memory": 4_u64 << 30,
            "PortBindings": {"22/tcp": [{"HostIp": "127.0.0.1", "HostPort": port}]}},
        "Mounts": [{"Type": "volume", "Name": format!("{name}-workspace"), "Destination": "/workspace"}],
        "State": {"Running": running, "StartedAt": if running { "2026-10-14T00:00:00Z" } else { "0001-01-01T00:00:00Z" }}
    })
}

impl Fake {
    fn host(cpus: u64, memory_gb: u64) -> Self {
        Self {
            cpus,
            memory_gb,
            ..Self::default()
        }
    }

    fn add(&self, id: &str, name: &str, owner: Option<&str>, port: u16, running: bool) {
        let value = container(id, name, owner, &port.to_string(), running, "foreign", &[]);
        self.containers.borrow_mut().insert(id.into(), value);
    }

    fn create(&self, args: &[String]) -> Result<String, Failure> {
        let name = flag(args, "--name");
        if self
            .containers
            .borrow()
            .values()
            .any(|c| c["Name"] == format!("/{name}"))
        {
            return Err(Failure::Refused(format!(
                "Conflict. The container name \"/{name}\" is already in use"
            )));
        }
        let mount = flag(args, "--mount").split(['=', ',']).nth(3).unwrap();
        self.volumes.borrow_mut().entry(mount.into()).or_insert(None);
        let port = flag(args, "--publish").split(':').nth(1).unwrap();
        let owner = flag(args, "--label").split_once('=').unwrap().1;
        let env: Vec<&str> = args
            .windows(2)
            .filter(|pair| pair[0] == "--env")
            .map(|pair| pair[1].as_str())
            .collect();
        let id = format!("{:064x}", self.containers.borrow().len() + 1);
        let mut value = container(&id, name, Some(owner), port, false, args.last().unwrap(), &env);
        let cpus: u64 = flag(args, "--cpus").parse().unwrap();
        value["HostConfig"]["NanoCpus"] = json!(cpus * 1_000_000_000);
        self.containers.borrow_mut().insert(id.clone(), value);
        Ok(id)
    }

    fn start(&self, id: &str) -> Result<String, Failure> {
        let mut containers = self.containers.borrow_mut();
        let port = |c: &Value| {
            c["HostConfig"]["PortBindings"]["22/tcp"][0]["HostPort"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let wanted = port(&containers[id]);
        let held = containers
            .iter()
            .any(|(other, c)| other != id && c["State"]["Running"] == true && port(c) == wanted);
        if held || self.blocked.borrow().contains(&wanted.parse().unwrap()) {
            return Err(Failure::Refused(format!(
                "Bind for 127.0.0.1:{wanted} failed: port is already allocated"
            )));
        }
        containers.get_mut(id).unwrap()["State"] = json!({"Running": true, "StartedAt": "2026-10-14T00:00:01Z"});
        Ok(String::new())
    }
}

impl Docker for Fake {
    fn docker(&self, args: &[String], timeout: Duration) -> Result<String, Failure> {
        self.calls.borrow_mut().push((args.join(" "), timeout));
        if self.stalled.contains(&args[..2].join(" ").as_str()) {
            return Err(Failure::Error(CloudError::Invalid("timed out")));
        }
        let labelled = |filter: &str, label: Option<&str>| {
            let wanted = filter.trim_start_matches("label=").strip_prefix(OWNER_LABEL).unwrap();
            label.is_some_and(|label| wanted.is_empty() || wanted == format!("={label}"))
        };
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        let (mut containers, mut volumes) = (self.containers.borrow_mut(), self.volumes.borrow_mut());
        match words.as_slice() {
            ["info", ..] => {
                return Ok(json!({"OSType": "linux", "NCPU": self.cpus, "MemTotal": self.memory_gb << 30}).to_string());
            }
            ["container", "ls", .., "--filter", filter, "--format", _] => {
                let ids = containers
                    .iter()
                    .filter(|(_, c)| labelled(filter, c["Config"]["Labels"][OWNER_LABEL].as_str()));
                return Ok(ids.map(|(id, _)| id.clone()).collect::<Vec<_>>().join("\n"));
            }
            ["container", "inspect", ids @ ..] => {
                return Ok(json!(ids.iter().map(|id| &containers[*id]).collect::<Vec<_>>()).to_string());
            }
            ["volume", "ls", "--filter", filter, "--format", _] => {
                let owned = volumes.iter().filter(|(_, label)| labelled(filter, label.as_deref()));
                return Ok(owned.map(|(name, _)| name.clone()).collect::<Vec<_>>().join("\n"));
            }
            ["pull", ..] => {}
            ["container", "stop", "--time", "30", id] => {
                containers.get_mut(*id).unwrap()["State"]["Running"] = json!(false);
            }
            ["container", "rm", "--force", "--volumes", id] => drop(containers.remove(*id)),
            ["volume", "create", "--label", owner, name] => {
                let owner = owner.split_once('=').unwrap().1.to_owned();
                volumes.entry((*name).into()).or_insert(Some(owner));
            }
            ["volume", "rm", name] => drop(volumes.remove(*name)),
            ["container", "create", ..] => {
                drop((containers, volumes));
                return self.create(args);
            }
            ["container", "start", id] => {
                drop((containers, volumes));
                return self.start(id);
            }
            other => panic!("unexpected docker call {other:?}"),
        }
        Ok(String::new())
    }
}

#[derive(Default)]
struct Cloud {
    spec: Option<WorkerSpec>,
    operation: Option<CreateState>,
    journal: Journal,
    operations: Vec<CreateState>,
    mutations: usize,
}

impl Records for Cloud {
    fn journal(&mut self, journal: &Journal) -> Result<(), CloudError> {
        self.journal = journal.clone();
        Ok(())
    }

    fn operation(&mut self, operation: &CreateState) -> Result<(), CloudError> {
        self.operations.push(operation.clone());
        Ok(())
    }

    fn mutation(&mut self) -> Result<(), CloudError> {
        self.mutations += 1;
        Ok(())
    }
}

fn host() -> Host {
    Host {
        name: "build-server".into(),
        access: Access::Ssh,
        address: Some("builder@build-server".into()),
        docker: Some("/usr/local/bin/docker".into()),
    }
}

impl Cloud {
    fn new(id: &str, cpu: u8) -> Self {
        let spec = serde_json::from_value(json!({
            "operation_id": id, "image_digest": format!("ghcr.io/example/worker@sha256:{}", "a".repeat(64)),
            "profile": {"provider": PROVIDER, "image": "ghcr.io/example/worker", "cpu": cpu, "memory_gb": 4},
            "public_key": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f",
            "registry_auth_id": null, "gpu_types": [], "cpu_flavors": ["build-server"], "data_centers": ["build-server"]
        }));
        Self {
            spec: Some(spec.unwrap()),
            operation: Some(CreateState::Prepared),
            ..Self::default()
        }
    }

    fn provision(&mut self, docker: &Fake) -> Result<crate::Worker, CloudError> {
        let (spec, mut operation, mut journal) = (
            self.spec.clone().unwrap(),
            self.operation.take().unwrap(),
            self.journal.clone(),
        );
        let worker = provision(docker, &host(), &spec, &mut operation, &mut journal, self);
        self.operation = Some(operation);
        worker
    }

    fn delete(&mut self, docker: &Fake) -> Result<(), CloudError> {
        let (id, mut operation, mut journal) = (self.id(), self.operation.take().unwrap(), self.journal.clone());
        let deleted = delete(docker, &host(), &id, &mut operation, &mut journal, self);
        self.operation = Some(operation);
        deleted
    }

    fn id(&self) -> String {
        self.spec.as_ref().unwrap().operation_id.clone()
    }

    fn worker_id(&self) -> String {
        let Some(CreateState::Bound { worker_id }) = &self.operation else {
            panic!("{:?}", self.operation)
        };
        worker_id.clone()
    }

    fn power(&self, docker: &Fake, on: bool) -> Result<(), CloudError> {
        power(docker, &self.id(), &self.worker_id(), on, &mut || Ok(()))
    }
}

fn port(worker: &crate::Worker) -> u16 {
    worker.ssh_endpoint().unwrap().port()
}

#[test]
fn clouds_on_one_host_get_their_own_names_volumes_and_ports() {
    let docker = Fake::host(16, 64);
    // Another Horizon's worker holds the port the second cloud tries first.
    docker.add(
        "other",
        "horizon-cloud-elsewhere",
        Some("elsewhere"),
        first_port("second"),
        false,
    );
    let (mut first, mut second) = (Cloud::new("first", 2), Cloud::new("second", 2));
    let (one, two) = (first.provision(&docker).unwrap(), second.provision(&docker).unwrap());
    assert_eq!(
        (one.name.as_str(), two.name.as_str()),
        ("horizon-cloud-first", "horizon-cloud-second")
    );
    let ports = [port(&one), port(&two)];
    assert!(
        ports[0] != ports[1] && !ports.contains(&first_port("second")),
        "{ports:?}"
    );
    assert_eq!(
        (one.status(), &one.docker_host),
        (WorkerStatus::Running, &Some(Box::new(host())))
    );
    assert_eq!(one.env["HORIZON_CLOUD_OPERATION"], "first");
    let volumes: Vec<String> = docker.volumes.borrow().keys().cloned().collect();
    assert_eq!(
        volumes,
        ["horizon-cloud-first-workspace", "horizon-cloud-second-workspace"]
    );
    assert_eq!(
        first.operations,
        [CreateState::Requested, first.operation.clone().unwrap()]
    );
    assert_eq!(first.journal.port, Some(ports[0]));
    // Every call is bounded; only the image pull may take long.
    for (command, timeout) in docker.calls.borrow().iter() {
        let limit = if command.starts_with("pull") { 30 * 60 } else { 90 };
        assert!(timeout.as_secs() <= limit, "{command}");
    }
}

#[test]
fn the_ssh_port_stays_pinned_across_stop_and_resume() {
    let docker = Fake::host(8, 16);
    let mut cloud = Cloud::new("pinned", 2);
    let worker = cloud.provision(&docker).unwrap();
    cloud.power(&docker, false).unwrap();
    let stopped = describe(
        &find(&docker, "pinned").unwrap().unwrap(),
        &host(),
        cloud.spec.as_ref().unwrap(),
    );
    assert_eq!(stopped.unwrap().status(), WorkerStatus::Stopped);
    cloud.power(&docker, true).unwrap();
    let resumed = cloud.provision(&docker).unwrap();
    assert_eq!(cloud.mutations, 0, "a running worker needs no request");
    assert_eq!((port(&resumed), &resumed.id), (port(&worker), &worker.id));
    // Another program took the port while the worker was stopped: resume says so.
    cloud.power(&docker, false).unwrap();
    docker.blocked.borrow_mut().push(port(&worker));
    let error = cloud.power(&docker, true).unwrap_err();
    assert!(
        matches!(error, CloudError::Rejected(_)) && error.to_string().contains("SSH port"),
        "{error}"
    );
}

#[test]
fn a_port_another_program_holds_is_skipped_and_a_created_worker_is_adopted() {
    let docker = Fake::host(8, 16);
    docker.blocked.borrow_mut().push(first_port("busy"));
    let worker = Cloud::new("busy", 2).provision(&docker).unwrap();
    assert_ne!(port(&worker), first_port("busy"));
    assert_eq!(
        docker.containers.borrow().len(),
        1,
        "the container that never started is gone"
    );
    // A crash between the create and the record: the next attempt adopts the container.
    let mut retry = Cloud::new("again", 2);
    retry.operation = Some(CreateState::Requested);
    retry.journal.port = Some(47_999);
    worker::create(&docker, retry.spec.as_ref().unwrap(), 47_999).unwrap();
    assert_eq!(port(&retry.provision(&docker).unwrap()), 47_999);
    assert_eq!(docker.containers.borrow().len(), 2);
}

#[test]
fn a_full_host_refuses_before_anything_is_created() {
    let docker = Fake::host(4, 8);
    docker.add("busy", "horizon-cloud-busy", Some("busy"), 47_500, true);
    let mut cloud = Cloud::new("big", 4);
    let error = cloud.provision(&docker).unwrap_err();
    assert!(matches!(error, CloudError::Capacity(_)), "{error}");
    assert!(error.to_string().contains("2.0 CPUs and 4.0 GB"), "{error}");
    assert!(docker.volumes.borrow().is_empty() && cloud.operations.is_empty());
    // A stopped worker holds no capacity.
    docker.containers.borrow_mut().get_mut("busy").unwrap()["State"]["Running"] = json!(false);
    cloud.provision(&docker).unwrap();
}

#[test]
fn delete_removes_only_the_clouds_own_container_and_volume() {
    let docker = Fake::host(16, 64);
    let (mut cloud, mut neighbour) = (Cloud::new("mine", 2), Cloud::new("neighbour", 2));
    cloud.provision(&docker).unwrap();
    neighbour.provision(&docker).unwrap();
    // Foreign resources, one with the cloud's own name but without its label.
    docker.add("foreign", "horizon-cloud-mine", None, 2222, true);
    docker.volumes.borrow_mut().insert("data".into(), None);
    // Another host's engine is never asked to delete this cloud.
    let elsewhere = Host {
        name: "elsewhere".into(),
        ..host()
    };
    let (mut operation, mut journal) = (cloud.operation.clone().unwrap(), cloud.journal.clone());
    assert!(
        delete(
            &docker,
            &elsewhere,
            "mine",
            &mut operation,
            &mut journal,
            &mut Cloud::default()
        )
        .is_err()
    );
    cloud.delete(&docker).unwrap();
    let containers: Vec<String> = docker.containers.borrow().keys().cloned().collect();
    assert_eq!(containers, [neighbour.worker_id(), "foreign".into()]);
    let volumes: Vec<String> = docker.volumes.borrow().keys().cloned().collect();
    assert_eq!(volumes, ["data", "horizon-cloud-neighbour-workspace"]);
    assert!(matches!(cloud.operation, Some(CreateState::Terminated { .. })));
    assert_eq!(cloud.journal, Journal::default());
    // A foreign volume with a cloud's volume name is never adopted.
    docker
        .volumes
        .borrow_mut()
        .insert("horizon-cloud-taken-workspace".into(), None);
    let error = Cloud::new("taken", 2).provision(&docker).unwrap_err();
    assert!(error.to_string().contains("did not create"), "{error}");
}

#[test]
fn a_timed_out_request_is_reported_and_a_deleting_cloud_stays_down() {
    let mut docker = Fake::host(8, 16);
    let mut cloud = Cloud::new("slow", 2);
    cloud.provision(&docker).unwrap();
    docker.stalled = vec!["container stop"];
    assert!(
        cloud
            .power(&docker, false)
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    assert!(find(&docker, "slow").unwrap().unwrap().running);
    docker.stalled = vec!["container rm"];
    assert!(cloud.delete(&docker).is_err());
    assert!(cloud.journal.deleting);
    let error = cloud.provision(&docker).unwrap_err();
    assert!(error.to_string().contains("deleting"), "{error}");
}

#[test]
fn host_settings_refuse_shell_syntax() {
    let mut host = host();
    host.validate().unwrap();
    for address in [
        "-oProxyCommand=x",
        "a b",
        "user@host;id",
        "$(id)",
        "@host",
        "user@",
        "-u@host",
    ] {
        host.address = Some(address.into());
        assert!(host.validate().is_err(), "{address}");
    }
    host.address = Some("build-server".into());
    for docker in ["docker", "/usr/bin/docker; id", "/opt/my docker", "/a/$(id)"] {
        host.docker = Some(docker.into());
        assert!(host.validate().is_err(), "{docker}");
    }
    for name in ["", "two words", &"x".repeat(64)] {
        host.name = name.into();
        assert!(host.validate().is_err(), "{name}");
    }
    let this_pc = |address: Option<&str>| Host {
        name: "this-pc".into(),
        access: Access::ThisPc,
        address: address.map(str::to_owned),
        docker: None,
    };
    assert!(this_pc(Some("build-server")).validate().is_err());
    assert_eq!(this_pc(None).proxy_command().unwrap(), None);
}

#[test]
fn workers_are_reached_through_the_host() {
    assert_eq!(
        host().proxy_command().unwrap().unwrap(),
        "ssh -o BatchMode=yes -o ConnectTimeout=10 -o ControlMaster=no -o ControlPath=none -W 127.0.0.1:%p -- 'builder@build-server'"
    );
    let tailnet = Host {
        access: Access::TailscaleSsh,
        ..host()
    };
    assert!(
        tailnet
            .proxy_command()
            .unwrap()
            .unwrap()
            .starts_with("tailscale ssh 'builder@build-server' ")
    );
    let command = host().command(&["ps".into()], std::path::Path::new("/unused")).unwrap();
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(command.get_program(), "ssh");
    assert_eq!(&args[args.len() - 3..args.len() - 1], ["--", "builder@build-server"]);
    let script = &args[args.len() - 1];
    assert!(
        script.starts_with("sh -c ") && script.contains("/usr/local/bin/docker"),
        "{script}"
    );
}

// A POSIX shell parses the remote command as the host's login shell would.
#[cfg(unix)]
#[test]
fn remote_arguments_reach_docker_unchanged() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let docker = root.path().join("docker");
    std::fs::write(&docker, "#!/bin/sh\nprintf '%s\\0' \"$@\"\n").unwrap();
    std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
    let host = Host {
        docker: Some(docker.to_string_lossy().into_owned()),
        ..host()
    };
    let hostile = [
        "it's",
        "$(touch pwned)",
        "a;b",
        "`id`",
        "two\nlines",
        "\"q\"",
        "*",
        "{{json .}}",
    ];
    let args: Vec<String> = hostile.iter().map(|arg| (*arg).to_owned()).collect();
    let command = host.command(&args, std::path::Path::new("/unused")).unwrap();
    let script = command.get_args().last().unwrap().to_owned();
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .env("HOME", root.path())
        .current_dir(root.path())
        .output()
        .unwrap();
    let received: Vec<String> = String::from_utf8(output.stdout)
        .unwrap()
        .split_terminator('\0')
        .map(str::to_owned)
        .collect();
    assert_eq!(received[..2], ["-H", "unix:///var/run/docker.sock"]);
    assert_eq!(received[2..], hostile);
    assert!(!root.path().join("pwned").exists());
    let config = std::fs::read_to_string(root.path().join(".horizon/docker-config/config.json")).unwrap();
    assert_eq!(config, PRIVATE_CONFIG, "no credential helper is ever detected");
}

// The bounded runner kills a command that outlives its time.
#[cfg(unix)]
#[test]
fn a_command_that_outlives_its_time_is_stopped() {
    let host = Host {
        name: "this-pc".into(),
        access: Access::ThisPc,
        address: None,
        docker: Some("/bin/sleep".into()),
    };
    let (cancel, config) = (crate::Cancellation::default(), tempfile::tempdir().unwrap());
    let engine = Engine {
        host: &host,
        local_config: config.path().join("docker"),
        cancel: &cancel,
    };
    let started = std::time::Instant::now();
    let error = CloudError::from(engine.docker(&["5".into()], Duration::from_millis(200)).unwrap_err());
    assert!(error.to_string().contains("did not answer in time"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(engine.docker(&["0".into()], Duration::from_secs(5)).is_ok());
    let written = std::fs::read_to_string(config.path().join("docker/config.json")).unwrap();
    assert_eq!(written, PRIVATE_CONFIG);
}

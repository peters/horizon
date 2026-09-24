use super::super::{
    Cancellation, CreateState, Stage,
    command::Runner,
    settings::Settings,
    ssh::Connection,
    state::{Deployment, Store, cloud_directory},
};
use super::{Catalog, Error, Result, Status};
use horizon_cloud_protocol::companion::{Request, Response};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::SocketAddr,
    path::Path,
    time::Duration,
};

#[derive(Clone)]
pub(super) struct Worker {
    pub id: String,
    pub revision: String,
    pub address: Option<SocketAddr>,
    pub status: Status,
}

pub(super) trait Transport {
    fn worker(&mut self, cloud: &str) -> Result<Option<Worker>>;
    fn call(&mut self, cloud: &str, request: &Request) -> Result<Response>;
    fn publish(&mut self, cloud: &str, catalog: &Catalog) -> Result<()>;
}

pub(super) struct Live<'a> {
    root: &'a Path,
    settings: &'a Settings,
    cancel: &'a Cancellation,
    states: BTreeMap<String, (Store, Option<Deployment>)>,
}

impl<'a> Live<'a> {
    pub fn new(root: &'a Path, settings: &'a Settings, cancel: &'a Cancellation) -> Self {
        Self {
            root,
            settings,
            cancel,
            states: BTreeMap::new(),
        }
    }

    fn load(&mut self, cloud: &str) -> Result<&Option<Deployment>> {
        self.cancel.check()?;
        if !self.states.contains_key(cloud) {
            let path = cloud_directory(self.root, cloud)?;
            let store = Store::lock(&path)?;
            let state = store.load()?;
            if state.as_ref().is_some_and(|state| state.cloud_id != cloud) {
                return Err(Error::Invalid("Companion deployment identity differs"));
            }
            self.states.insert(cloud.into(), (store, state));
        }
        self.states
            .get(cloud)
            .map(|(_, state)| state)
            .ok_or(Error::Invalid("Missing companion state"))
    }

    fn exchange(
        &mut self,
        cloud: &str,
        command: &str,
        payload: &impl serde::Serialize,
        timeout: Duration,
    ) -> Result<String> {
        let state = self
            .load(cloud)?
            .as_ref()
            .ok_or(Error::Invalid("Companion cloud has no worker"))?;
        let worker = state
            .worker
            .clone()
            .ok_or(Error::Invalid("Companion worker is missing"))?;
        let connection = Connection::new(&worker, self.settings, &cloud_directory(self.root, cloud)?)?;
        // Companion access extends an already verified owner connection; never establish new trust here.
        let mut args = connection.args();
        for arg in &mut args {
            if arg == "StrictHostKeyChecking=accept-new" {
                *arg = "StrictHostKeyChecking=yes".into();
            }
        }
        let mut ssh = std::process::Command::new("ssh");
        ssh.args(args).arg(command);
        let directory = tempfile::tempdir()?;
        let input = directory.path().join("request.json");
        let output = directory.path().join("response.json");
        let mut file = tempfile::NamedTempFile::new_in(directory.path())?;
        serde_json::to_writer(&mut file, payload).map_err(|_| Error::Json)?;
        file.flush()?;
        file.persist(&input).map_err(|error| error.error)?;
        let runner = Runner {
            cancel: self.cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        runner
            .to_file("Companion SSH operation", &mut ssh, &input, &output, timeout)
            .map_err(|_| Error::Invalid("Companion SSH command failed; check connectivity and worker image support"))?;
        let mut response = String::new();
        std::fs::File::open(output)?
            .take(256 * 1024 + 1)
            .read_to_string(&mut response)?;
        if response.len() > 256 * 1024 {
            return Err(Error::Invalid("Companion response is too large"));
        }
        Ok(response)
    }
}

impl Transport for Live<'_> {
    fn worker(&mut self, cloud: &str) -> Result<Option<Worker>> {
        let Some(state) = self.load(cloud)? else {
            return Ok(None);
        };
        let Some(worker) = &state.worker else { return Ok(None) };
        let bound = matches!(&state.operation, CreateState::Bound { worker_id } if worker_id == &worker.id);
        let status = if state.stop_requested
            || matches!(state.stage, Stage::Stopped | Stage::Stopping)
            || worker.desired_status == "EXITED"
        {
            Status::Stopped
        } else if bound
            && state.stage == Stage::Ready
            && state.source_ready
            && worker.desired_status == "RUNNING"
            && worker.ssh_address().is_some()
        {
            Status::Ready
        } else {
            Status::Unavailable
        };
        Ok(Some(Worker {
            id: worker.id.clone(),
            revision: state.revision.clone(),
            address: worker.ssh_address(),
            status,
        }))
    }

    fn call(&mut self, cloud: &str, request: &Request) -> Result<Response> {
        let timeout = if matches!(request, Request::Authorize { .. }) {
            // Two 300-second checkout phases plus bounded worktree and host-key verification.
            Duration::from_secs(690)
        } else {
            Duration::from_secs(45)
        };
        serde_json::from_str(&self.exchange(cloud, "horizon-cloud-worker companion-control", request, timeout)?)
            .map_err(|_| Error::Invalid("Worker returned an unsupported companion response"))
    }

    fn publish(&mut self, cloud: &str, catalog: &Catalog) -> Result<()> {
        catalog.validate().map_err(Error::Invalid)?;
        self.exchange(
            cloud,
            "horizon-cloud-worker companions publish",
            catalog,
            Duration::from_secs(45),
        )?;
        Ok(())
    }
}

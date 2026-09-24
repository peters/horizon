//! Production steps: Git, Docker and the registry build the image, the provider API
//! switches the worker, and SSH releases hosted devices.
use super::{Head, Provider, Steps, pair};
use crate::cloud_runtime::{
    Error, Event, Result, browser_auth,
    command::Runner,
    deployment::{Request, storage},
    git_auth,
    image::Images,
    registry, repository,
    ssh::Connection,
    state::{Deployment, ReplacementImage, Store},
};
use horizon_cloud::{
    Cancellation, CloudError, CreateState, WorkerSpec,
    runpod::{RunPod, replacement::Observed},
};
use std::{
    cell::RefCell,
    path::Path,
    time::{Duration, Instant},
};

const OBSERVE_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Observes and switches the bound worker's image through the provider.
pub(super) struct Pod<'a> {
    provider: &'a RunPod,
    store: &'a Store,
    cancel: &'a Cancellation,
}

impl<'a> Pod<'a> {
    pub(super) const fn new(provider: &'a RunPod, store: &'a Store, cancel: &'a Cancellation) -> Self {
        Self {
            provider,
            store,
            cancel,
        }
    }
}

/// A provider read waits at most `OBSERVE_TIMEOUT` and never past `deadline`; `None`
/// once the deadline has passed.
pub(super) fn observe_timeout(deadline: Option<Instant>, now: Instant) -> Option<Duration> {
    let timeout = deadline.map_or(OBSERVE_TIMEOUT, |deadline| {
        deadline.saturating_duration_since(now).min(OBSERVE_TIMEOUT)
    });
    (!timeout.is_zero()).then_some(timeout)
}

impl Provider for Pod<'_> {
    fn replace(&self, worker_id: &str, from: &WorkerSpec, to: &WorkerSpec) -> Result<()> {
        Ok(self.provider.replace_image(from, to, worker_id, self.cancel)?)
    }

    fn observe(&self, state: &Deployment, deadline: Option<Instant>) -> Result<Observed> {
        let timeout = observe_timeout(deadline, Instant::now()).ok_or(CloudError::Transport)?;
        let (worker_id, current, next) = pair(state)?;
        let volume = storage::expected_owned(self.store, state)?;
        Ok(self
            .provider
            .observe_image(&worker_id, &current, &next, volume.as_ref(), self.cancel, timeout)?)
    }

    fn pause(&self, deadline: Instant) -> Result<bool> {
        let until = deadline.min(Instant::now() + POLL_INTERVAL);
        while let Some(left) = until
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
        {
            self.cancel.check()?;
            std::thread::sleep(left.min(Duration::from_millis(100)));
        }
        Ok(Instant::now() < deadline)
    }
}

/// Everything a rebuild does outside its record, for one locked cloud.
pub(super) struct Live<'a> {
    request: &'a Request,
    store: &'a Store,
    provider: RunPod,
    runner: Runner<'a>,
    /// Holds the registry generation lock until the provider update is sent.
    registry: RefCell<Option<registry::Prepared>>,
    generation: Option<String>,
    git_auth: bool,
}

impl<'a> Live<'a> {
    /// Loads the registry grants the image needs; `building` also requires the push grant.
    pub(super) fn new(
        request: &'a Request,
        store: &'a Store,
        state: &Deployment,
        building: bool,
        cancel: &'a Cancellation,
        emit: &'a dyn Fn(Event),
    ) -> Result<Self> {
        let settings = &request.settings;
        let binding = settings
            .registries
            .as_ref()
            .map(|config| config.select(&state.profile.image))
            .transpose()?
            .flatten();
        if state.registry_generation.is_some() && binding.is_none() {
            return Err(Error::Invalid(
                "Private image registry binding is missing; restore or rotate it before retrying",
            ));
        }
        let registry =
            registry::Prepared::for_image(settings, &state.profile.image, Some(&state.repository), building)?;
        Ok(Self {
            request,
            store,
            provider: RunPod::new(settings.credential()?),
            runner: Runner {
                cancel,
                emit,
                secrets: registry.as_ref().map_or_else(Vec::new, registry::Prepared::redactions),
            },
            registry: RefCell::new(registry),
            generation: binding.map(|binding| binding.generation.clone()),
            git_auth: git_auth::Prepared::for_repository(&settings.git_credentials, &state.repository)?.is_some(),
        })
    }

    fn pod(&self) -> Pod<'_> {
        Pod::new(&self.provider, self.store, self.runner.cancel)
    }

    fn images<'s>(&'s self, registry: Option<&'s registry::Prepared>, building: bool) -> Images<'s> {
        Images {
            docker_host: self.request.settings.docker_host.as_deref(),
            isolated_registry: registry.is_some(),
            docker_config: registry.map_or(self.request.settings.docker_config.as_path(), |registry| {
                registry.docker_config(building)
            }),
            runner: &self.runner,
        }
    }

    /// The image with the pull binding the worker will use, verified to read it.
    fn register(&self, state: &Deployment, digest: String) -> Result<ReplacementImage> {
        let mut registry = self.registry.borrow_mut();
        let Some(registry) = registry.as_mut() else {
            let spec = state
                .spec
                .as_ref()
                .ok_or(Error::Invalid("Missing worker specification"))?;
            return Ok(ReplacementImage {
                digest,
                registry_auth_id: spec.registry_auth_id.clone(),
                registry_generation: None,
            });
        };
        registry.verify_image(&digest, self.runner.cancel)?;
        let registry_auth_id = Some(registry.ensure_provider(&self.provider, self.runner.cancel)?);
        Ok(ReplacementImage {
            digest,
            registry_auth_id,
            registry_generation: self.generation.clone(),
        })
    }
}

impl Provider for Live<'_> {
    fn replace(&self, worker_id: &str, from: &WorkerSpec, to: &WorkerSpec) -> Result<()> {
        self.pod().replace(worker_id, from, to)
    }

    fn observe(&self, state: &Deployment, deadline: Option<Instant>) -> Result<Observed> {
        self.pod().observe(state, deadline)
    }

    fn pause(&self, deadline: Instant) -> Result<bool> {
        self.pod().pause(deadline)
    }
}

impl Steps for Live<'_> {
    fn head(&self, repository: &Path) -> Result<Head> {
        let revision = repository::resolve_with_runner(repository, "HEAD", &self.runner)?;
        let config = repository::launch::committed_config(repository, &revision, &self.runner)?;
        Ok(Head { revision, config })
    }

    fn build(&self, state: &Deployment, revision: &str, tag: &str) -> Result<ReplacementImage> {
        let root = tempfile::tempdir_in(self.store.root())?;
        let source = repository::snapshot(&state.repository, revision, root.path(), &self.runner)?;
        let digest = {
            let registry = self.registry.borrow();
            let digest =
                self.images(registry.as_ref(), true)
                    .prepare_tagged(&state.profile, &source, &state.cloud_id, tag)?;
            if self.git_auth {
                self.images(registry.as_ref(), false).validate_contract(
                    &digest,
                    &state.cloud_id,
                    &state.profile.capabilities,
                    true,
                )?;
            }
            digest
        };
        self.register(state, digest)
    }

    fn verify(&self, state: &Deployment, image: &ReplacementImage) -> Result<()> {
        if self.register(state, image.digest.clone())? != *image {
            return Err(Error::Invalid(
                "The image's registry binding changed after it was built; cancel the replacement and rebuild",
            ));
        }
        Ok(())
    }

    fn release_devices(&self, state: &Deployment) -> Result<()> {
        let (CreateState::Bound { worker_id }, Some(spec)) = (&state.operation, &state.spec) else {
            return Err(Error::Invalid("Only a bound worker's image can be replaced"));
        };
        let worker = self
            .provider
            .inspect(worker_id, self.runner.cancel)?
            .ok_or(CloudError::WorkerLost)?;
        worker.verify(spec)?;
        let connection = Connection::new(&worker, &self.request.settings, self.store.root())?;
        browser_auth::revoke(&connection, &self.runner)
    }
}

//! Worker image preparation and the image contract checked before allocation.
use super::{Deployment, Request, Result, Runner, Store, repository, sizing::cpu_flavors};
use crate::cloud_runtime::image::Images;
use horizon_cloud::{CreateState, WorkerSpec};
use std::path::PathBuf;

pub(super) fn validate_allocation_image(
    request: &Request,
    state: &Deployment,
    spec: &WorkerSpec,
    git_auth: bool,
    runner: &Runner<'_>,
    registry: Option<&crate::cloud_runtime::registry::Prepared>,
) -> Result<()> {
    if state.operation == CreateState::Prepared {
        Images {
            docker_host: request.settings.docker_host.as_deref(),
            isolated_registry: registry.is_some(),
            docker_config: registry.map_or(request.settings.docker_config.as_path(), |registry| {
                registry.docker_config(false)
            }),
            runner,
        }
        .validate_contract(&spec.image_digest, &state.cloud_id, &state.profile, git_auth)?;
    }
    Ok(())
}

pub(super) fn prepare_image(
    request: &Request,
    store: &Store,
    runner: &Runner<'_>,
    state: &mut Deployment,
    registry: Option<&crate::cloud_runtime::registry::Prepared>,
) -> Result<()> {
    let build_root = tempfile::tempdir_in(store.root())?;
    let source = if state.profile.build.is_some() {
        repository::snapshot(&state.repository, &state.revision, build_root.path(), runner)?
    } else {
        state.repository.clone()
    };
    let images = Images {
        docker_host: request.settings.docker_host.as_deref(),
        isolated_registry: registry.is_some(),
        docker_config: registry.map_or(request.settings.docker_config.as_path(), |registry| {
            registry.docker_config(state.profile.build.is_some())
        }),
        runner,
    };
    let digest = images.prepare(&state.profile, &source, &state.cloud_id)?;
    let public_key = std::fs::read_to_string(PathBuf::from(format!(
        "{}.pub",
        request.settings.ssh_identity_file.display()
    )))?
    .trim()
    .to_owned();
    state.spec = Some(WorkerSpec {
        operation_id: state.cloud_id.clone(),
        image_digest: digest,
        profile: state.profile.clone(),
        public_key,
        registry_auth_id: request.settings.registry_pull_auth_id.clone(),
        gpu_types: request.settings.gpu_types.clone(),
        cpu_flavors: cpu_flavors(&state.profile, &request.settings)?,
        data_centers: request.settings.data_centers.clone(),
        startup_metadata: None,
    });
    store.save(state)
}

//! Worker image preparation and the image contract checked before allocation.
use super::{Deployment, Request, Result, Runner, Store, repository, sizing::cpu_flavors};
use crate::cloud_runtime::{
    git_auth,
    image::{Images, Layer, default_tag},
    siblings,
};
use horizon_cloud::{Build, CreateState, WorkerSpec};
use std::path::{Path, PathBuf};

pub(super) fn validate_allocation_image(
    request: &Request,
    state: &Deployment,
    spec: &WorkerSpec,
    git_auth: bool,
    runner: &Runner<'_>,
    registry: Option<&crate::cloud_runtime::registry::Prepared>,
) -> Result<()> {
    if state.operation == CreateState::Prepared {
        let images = Images {
            docker_host: request.settings.docker_host.as_deref(),
            isolated_registry: registry.is_some(),
            docker_config: registry.map_or(request.settings.docker_config.as_path(), |registry| {
                registry.docker_config(false)
            }),
            runner,
        };
        match &state.siblings {
            None => images.validate_contract(&spec.image_digest, &state.cloud_id, &state.profile, git_auth)?,
            Some(set) => images.validate_siblings_contract(
                &spec.image_digest,
                &state.cloud_id,
                &state.profile,
                sibling_grants(request, state, set)?,
            )?,
        }
    }
    Ok(())
}

/// A cloud with siblings receives version 2 Git grants when any repository of the set
/// has a binding.
fn sibling_grants(request: &Request, state: &Deployment, set: &siblings::Set) -> Result<bool> {
    if request.settings.git_credentials.is_empty() {
        return Ok(false);
    }
    let siblings = set.grant_siblings()?;
    Ok(!git_auth::select(&request.settings.git_credentials, &state.repository, &siblings)?.is_empty())
}

/// Each sibling's committed snapshot and recipe, in layering order.
fn sibling_sources(
    state: &Deployment,
    set: &siblings::Set,
    root: &Path,
    runner: &Runner<'_>,
) -> Result<Vec<(Build, PathBuf)>> {
    let primary = state
        .profile
        .build
        .as_ref()
        .ok_or(siblings::SiblingError::PrimaryImageOnly)?;
    set.members
        .iter()
        .enumerate()
        .map(|(index, sibling)| {
            let checkout = sibling.checkout()?;
            let recipe = sibling.recipe(primary, runner)?;
            let root = root.join(format!("sibling-{index}"));
            std::fs::create_dir(&root)?;
            let source = repository::snapshot(checkout, &sibling.revision, &root, runner)?;
            Ok((recipe, source))
        })
        .collect()
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
    let digest = if let Some(set) = &state.siblings {
        let root = build_root.path().join("siblings");
        std::fs::create_dir(&root)?;
        let sources = sibling_sources(state, set, &root, runner)?;
        let layers: Vec<_> = set
            .members
            .iter()
            .zip(&sources)
            .map(|(sibling, (build, source))| Layer {
                alias: &sibling.alias,
                build,
                source,
            })
            .collect();
        images.prepare_layered(
            &state.profile,
            &source,
            &layers,
            &state.cloud_id,
            &default_tag(&state.cloud_id),
            sibling_grants(request, state, set)?,
        )?
    } else {
        images.prepare(&state.profile, &source, &state.cloud_id)?
    };
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

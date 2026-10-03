//! Worker image preparation and the image contract checked before allocation.
use super::{Deployment, Request, Result, Runner, Store, repository};
use crate::cloud_runtime::{
    WorkerContract, git_auth,
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
) -> Result<Option<WorkerContract>> {
    if state.operation == CreateState::Prepared {
        let images = Images {
            docker_host: request.settings.docker_host.as_deref(),
            isolated_registry: registry.is_some(),
            docker_config: registry.map_or(request.settings.docker_config.as_path(), |registry| {
                registry.docker_config(false)
            }),
            runner,
        };
        return match &state.siblings {
            None => images.validate_contract(&spec.image_digest, &state.cloud_id, &state.profile, git_auth),
            Some(set) => images.validate_siblings_contract(
                &spec.image_digest,
                &state.cloud_id,
                &state.profile,
                sibling_grants(request, state, set)?,
            ),
        }
        .and_then(|contract| {
            if crate::cloud_runtime::tailnet::Selection::load(&request.state_root)
                .map_err(|_| super::super::Error::Invalid("Invalid tailnet selection"))?
                .tailnet
                .is_some()
                && !contract.tailnet
            {
                return Err(super::super::Error::Invalid(
                    "This image cannot isolate tailnet credentials; rebuild with the current cloud worker",
                ));
            }
            Ok(Some(contract))
        });
    }
    Ok(None)
}

/// A cloud with siblings receives version 2 Git grants when any repository of the set
/// has a binding.
pub(super) fn sibling_grants(request: &Request, state: &Deployment, set: &siblings::Set) -> Result<bool> {
    if request.settings.git_credentials.is_empty() {
        return Ok(false);
    }
    let siblings = set.grant_siblings()?;
    Ok(!git_auth::select(&request.settings.git_credentials, &state.repository, &siblings)?.is_empty())
}

/// Builds the primary snapshot `source` with each of the deployment's siblings' committed
/// snapshot and recipe at `revisions`, in layering order, layered on it under `tag`,
/// extracting the snapshots below `root`.
pub(super) fn build_layered(
    request: &Request,
    images: &Images<'_>,
    state: &Deployment,
    revisions: &[String],
    source: &Path,
    root: &Path,
    tag: &str,
) -> Result<String> {
    let set = state.siblings.as_ref().ok_or(crate::cloud_runtime::Error::Invalid(
        "This cloud has no same-worker siblings",
    ))?;
    let primary = state
        .profile
        .build
        .as_ref()
        .ok_or(siblings::SiblingError::PrimaryImageOnly)?;
    if revisions.len() != set.members.len() {
        return Err(crate::cloud_runtime::Error::Invalid(
            "The image needs one revision per same-worker sibling",
        ));
    }
    let sources = set
        .members
        .iter()
        .zip(revisions)
        .enumerate()
        .map(|(index, (sibling, revision))| {
            let checkout = sibling.checkout()?;
            let recipe = sibling.recipe(revision, primary, images.runner)?;
            let root = root.join(format!("sibling-{index}"));
            std::fs::create_dir(&root)?;
            let source = repository::snapshot(checkout, revision, &root, images.runner)?;
            Ok((recipe, source))
        })
        .collect::<Result<Vec<(Build, PathBuf)>>>()?;
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
        source,
        &layers,
        &state.cloud_id,
        tag,
        sibling_grants(request, state, set)?,
    )
}

pub(super) fn prepare_image(
    request: &Request,
    store: &Store,
    runner: &Runner<'_>,
    state: &mut Deployment,
    registry: Option<&crate::cloud_runtime::registry::Prepared>,
) -> Result<()> {
    if let Some(registry) = registry {
        registry.preflight(runner)?;
    }
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
        let revisions: Vec<_> = set.members.iter().map(|sibling| sibling.revision.clone()).collect();
        build_layered(
            request,
            &images,
            state,
            &revisions,
            &source,
            &root,
            &default_tag(&state.cloud_id),
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
    let cpu_flavors = super::sizing::bound_types(&state.profile, &request.settings, state.spec.as_ref())?;
    let data_centers = super::sizing::bound_centers(&state.profile, &request.settings, state.spec.as_ref())?;
    let exact_placement = state.spec.as_ref().is_some_and(|spec| spec.exact_placement)
        || request
            .settings
            .placement
            .as_ref()
            .is_some_and(|placement| !placement.cpu_types.is_empty());
    state.spec = Some(WorkerSpec {
        exact_placement,
        operation_id: state.cloud_id.clone(),
        image_digest: digest,
        profile: state.profile.clone(),
        public_key,
        registry_auth_id: horizon_cloud::provider::Description::of(&state.profile)
            .registry_auth
            .then(|| request.settings.registry_pull_auth_id.clone())
            .flatten(),
        gpu_types: request.settings.gpu_types.clone(),
        cpu_flavors,
        data_centers,
        startup_metadata: None,
    });
    store.save(state)
}

//! Worker image preparation and the image contract checked before allocation.
use super::{Deployment, Error, Request, Result, Runner, Store, repository};
use crate::cloud_runtime::{
    WorkerContract, git_auth,
    github::publish::Publisher,
    image::{Context, Images, Layer, default_tag},
    progress::Progress,
    repository::uncommitted,
    siblings,
};
use horizon_cloud::{Build, CreateState, WorkerSpec};
use std::path::{Path, PathBuf};

const CHANGED: &str = "The uncommitted files in the image's build context are not the ones confirmed when this cloud was created. Put them back as they were, or delete this cloud and create a new one to confirm the files as they are now";

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
            publisher: None,
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
            let selected = crate::cloud_runtime::tailnet::Selection::load(&request.state_root)
                .map_err(|_| super::super::Error::Invalid("Invalid tailnet selection"))?;
            crate::cloud_runtime::tailnet::validate_worker_contract(&selected, &contract)?;
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

/// Builds the primary snapshot of `context` with each of the deployment's siblings'
/// committed snapshot and recipe at `revisions`, in layering order, layered on it under
/// `tag`, extracting the snapshots below `root`.
pub(super) fn build_layered(
    request: &Request,
    images: &Images<'_>,
    state: &Deployment,
    revisions: &[String],
    context: Context<'_>,
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
        context,
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
    let source = if let Some(build) = &state.profile.build {
        let source = repository::snapshot(&state.repository, &state.revision, build_root.path(), runner)?;
        working_copy(request, state, build, &source, runner)?;
        source
    } else {
        state.repository.clone()
    };
    let context = Context {
        source: &source,
        uncommitted: state.uncommitted_image.as_ref(),
    };
    let images = Images {
        docker_host: request.settings.docker_host.as_deref(),
        isolated_registry: registry.is_some(),
        docker_config: registry.map_or(request.settings.docker_config.as_path(), |registry| {
            registry.docker_config(state.profile.build.is_some())
        }),
        runner,
        // A bound image repository publishes with its own credential.
        publisher: registry.is_none().then_some(Publisher {
            docker_config: &request.settings.docker_config,
            cloud_id: &state.cloud_id,
        }),
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
            context,
            &root,
            &default_tag(&state.cloud_id),
        )?
    } else {
        images.prepare_layered(
            &state.profile,
            context,
            &[],
            &state.cloud_id,
            &default_tag(&state.cloud_id),
            false,
        )?
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
        registry_auth_id: pull_auth(&state.profile, &request.settings),
        gpu_types: request.settings.gpu_types.clone(),
        cpu_flavors,
        data_centers,
        startup_metadata: None,
    });
    store.save(state)
}

/// Lays the uncommitted files that a person confirmed in New cloud, as the record holds
/// their hash, over `snapshot`. Also without confirmed files, so one added since is refused.
fn working_copy(
    request: &Request,
    state: &Deployment,
    build: &Build,
    snapshot: &Path,
    runner: &Runner<'_>,
) -> Result<()> {
    if request.working_copy.is_none() && state.uncommitted_image.is_none() {
        return Ok(());
    }
    let confirmed = state.uncommitted_image.as_ref().map(|image| image.hash.as_str());
    if confirmed.is_some() {
        (runner.emit)(crate::cloud_runtime::Event::Progress(Progress::activity(
            "Adding the confirmed uncommitted files to the build context",
        )));
    }
    runner.cancel.check()?;
    uncommitted::overlay(&state.repository, &state.revision, build, confirmed, snapshot).map_err(|error| {
        if uncommitted::changed(&error) {
            Error::Invalid(CHANGED)
        } else {
            error
        }
    })
}

/// The provider registry login for the image of `profile`. The public base image gets
/// none: anyone can pull it, and a stale saved login could make the provider refuse it.
fn pull_auth(profile: &horizon_cloud::Profile, settings: &crate::cloud_runtime::settings::Settings) -> Option<String> {
    (horizon_cloud::provider::Description::of(profile).registry_auth
        && !crate::cloud_runtime::repository::launch::quick_start::is_public_base(&profile.image))
    .then(|| settings.registry_pull_auth_id.clone())
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_runtime::repository::launch::quick_start;
    /// New cloud's working-copy build; `deployment::prepare` is tested on Unix.
    #[cfg(unix)]
    mod from_working_copy {
        use super::*;
        use crate::cloud_runtime::{
            Cancellation, Error,
            repository::uncommitted::{Confirmed, collect},
        };
        use std::process::Command;

        fn git(repository: &Path, args: &[&str]) -> String {
            let output = Command::new("git")
                .arg("-C")
                .arg(repository)
                .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid"])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        }

        fn write(root: &Path, path: &str, text: &str) {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        /// A New cloud request for a checkout with a committed recipe in `.horizon`.
        fn request(root: &Path) -> Request {
            let repository = root.join("repository");
            std::fs::create_dir(&repository).unwrap();
            git(&repository, &["init", "--quiet"]);
            write(&repository, ".horizon/Dockerfile", "FROM scratch\n");
            git(&repository, &["add", "."]);
            git(&repository, &["commit", "--quiet", "-m", "Recipe"]);
            let revision = git(&repository, &["rev-parse", "HEAD"]);
            let profile = serde_json::from_value(serde_json::json!({
                "provider":"runpod","image":"registry.example.com/worker","cpu":4,"memory_gb":8,
                "build":{"context":".horizon","dockerfile":".horizon/Dockerfile"}
            }))
            .unwrap();
            let settings = serde_json::from_value(serde_json::json!({
                "runpod_key_file":"unused","ssh_identity_file":"unused","docker_config":"unused",
                "registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]
            }))
            .unwrap();
            Request::new(
                "working-copy".into(),
                repository.canonicalize().unwrap(),
                revision,
                profile,
                root.join("state"),
                settings,
            )
        }

        /// Lays the working copy over a fresh snapshot of `.horizon` and returns its Dockerfile.
        fn built(request: &Request) -> Result<String> {
            let store = Store::lock(&request.state_root)?;
            let state = store.load()?.unwrap();
            let snapshot = tempfile::tempdir().unwrap();
            write(snapshot.path(), ".horizon/Dockerfile", "FROM scratch\n");
            let cancel = Cancellation::default();
            let runner = Runner {
                cancel: &cancel,
                emit: &|_| {},
                secrets: Vec::new(),
            };
            working_copy(
                request,
                &state,
                request.profile.build.as_ref().unwrap(),
                snapshot.path(),
                &runner,
            )?;
            Ok(std::fs::read_to_string(snapshot.path().join(".horizon/Dockerfile")).unwrap())
        }

        #[test]
        fn new_cloud_records_and_builds_exactly_the_confirmed_uncommitted_files() {
            let root = tempfile::tempdir().unwrap();
            let mut request = request(root.path());
            let repository = request.repository.clone();
            write(&repository, ".horizon/Dockerfile", "FROM scratch\nCOPY tool /\n");
            let files = collect(&repository, &request.revision, request.profile.build.as_ref().unwrap()).unwrap();
            let hash = files.hash().unwrap();
            request.working_copy = Some(Confirmed {
                hash: Some(hash.clone()),
            });
            crate::cloud_runtime::deployment::prepare(&request).unwrap();
            let recorded = Store::lock(&request.state_root).unwrap().load().unwrap().unwrap();
            let image = recorded.uncommitted_image.unwrap();
            assert_eq!(
                (image.revision.as_str(), image.hash.as_str()),
                (request.revision.as_str(), hash.as_str())
            );
            assert_eq!(built(&request).unwrap(), "FROM scratch\nCOPY tool /\n");
            // A reconnect names no confirmation; the record keeps building the same files.
            let mut reconnect = request.clone();
            reconnect.working_copy = None;
            crate::cloud_runtime::deployment::prepare(&reconnect).unwrap();
            assert_eq!(built(&reconnect).unwrap(), "FROM scratch\nCOPY tool /\n");
            // A retry that confirmed other files is refused.
            let mut other = request.clone();
            other.working_copy = Some(Confirmed {
                hash: Some("f".repeat(64)),
            });
            assert!(crate::cloud_runtime::deployment::prepare(&other).is_err());
            // A file that changed after the confirmation refuses the build.
            write(&repository, ".horizon/Dockerfile", "FROM scratch\nCOPY other /\n");
            let changed = built(&request).unwrap_err().to_string();
            assert!(
                changed.contains("not the ones confirmed when this cloud was created"),
                "{changed}"
            );
            // Once the image is built, a rebuild can record other files; a reconnect from the
            // card still names the files confirmed at creation.
            let store = Store::lock(&request.state_root).unwrap();
            let mut rebuilt = store.load().unwrap().unwrap();
            rebuilt.spec = Some(WorkerSpec {
                exact_placement: false,
                operation_id: rebuilt.cloud_id.clone(),
                image_digest: format!("registry.example.com/worker@sha256:{}", "a".repeat(64)),
                profile: rebuilt.profile.clone(),
                public_key: "fixture".into(),
                registry_auth_id: None,
                gpu_types: Vec::new(),
                cpu_flavors: Vec::new(),
                data_centers: Vec::new(),
                startup_metadata: None,
            });
            rebuilt.uncommitted_image = None;
            store.save(&rebuilt).unwrap();
            drop(store);
            crate::cloud_runtime::deployment::prepare(&request).unwrap();
        }

        #[test]
        fn new_cloud_with_nothing_uncommitted_refuses_a_file_added_later_and_a_commit_build_ignores_it() {
            let root = tempfile::tempdir().unwrap();
            let mut request = request(root.path());
            request.working_copy = Some(Confirmed::default());
            crate::cloud_runtime::deployment::prepare(&request).unwrap();
            let recorded = Store::lock(&request.state_root).unwrap().load().unwrap().unwrap();
            assert_eq!(
                recorded.uncommitted_image, None,
                "the image builds from the commit alone"
            );
            assert_eq!(built(&request).unwrap(), "FROM scratch\n");
            write(&request.repository, ".horizon/added.sh", "echo added\n");
            assert!(matches!(built(&request), Err(Error::Invalid(message)) if message == CHANGED));
            // A typed revision builds its commit and never reads the working copy.
            request.working_copy = None;
            assert_eq!(built(&request).unwrap(), "FROM scratch\n");
        }
    }

    #[test]
    fn a_saved_pull_login_never_reaches_the_public_base_image() {
        let settings: crate::cloud_runtime::settings::Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file": "/absent", "ssh_identity_file": "/absent", "docker_config": "/absent",
            "registry_pull_auth_id": "saved-pull-login", "cpu_flavors": [], "gpu_types": []
        }))
        .unwrap();
        let config = quick_start::builtin().unwrap();
        let mut profile = config.profiles[quick_start::PROFILE].clone();
        assert_eq!(pull_auth(&profile, &settings), None);
        profile.image = "registry.example/team/worker@sha256:".to_owned() + &"a".repeat(64);
        assert_eq!(pull_auth(&profile, &settings).as_deref(), Some("saved-pull-login"));
    }
}

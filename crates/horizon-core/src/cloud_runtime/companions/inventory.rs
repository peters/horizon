//! Read committed declarations and local repository identity off the UI thread.
use super::super::{Cancellation, command::Runner, repository};
use super::{Context, Declaration, Error, Owner, Result, Target};
use crate::cloud_panel::CloudGroups;
use std::{collections::BTreeSet, path::Path, process::Command, time::Duration};

/// # Errors
/// The source must remain in the current owning workspace. A source commit without
/// `.horizon/cloud.yml` declares no companions; an unreadable one is an error.
pub fn prepare(owner: &Owner, groups: &CloudGroups, cancel: &Cancellation) -> Result<Context> {
    let runner = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let mut inventory = Vec::new();
    let mut source = None;
    let mut ids = BTreeSet::new();
    for group in &groups.0 {
        if group.workspace != owner.scope.workspace_id {
            continue;
        }
        let Some(launch) = &group.remote else { continue };
        cancel.check()?;
        if !ids.insert(&launch.id) {
            return Err(Error::Invalid("Cloud identity is ambiguous"));
        }
        let repository = match identity(&group.cwd, &runner) {
            Ok(repository) => repository,
            Err(error) if launch.id == owner.cloud_id => return Err(error),
            Err(_) => continue,
        };
        let target = Target {
            scope: owner.scope.clone(),
            cloud_id: launch.id.clone(),
            declaration: Declaration::new(repository, launch.profile_name.clone()),
        };
        if launch.id == owner.cloud_id {
            let declarations =
                match repository::launch::prepare(&group.cwd.to_string_lossy(), &launch.revision, &runner) {
                    Ok(prepared) => prepared
                        .config
                        .cloud_companions()
                        .map(|(alias, declaration)| (alias.to_owned(), declaration.clone()))
                        .collect(),
                    // A commit without settings, as a quick start runs, declares no companions.
                    Err(error) if repository::launch::is_missing_config(&error) => std::collections::BTreeMap::new(),
                    Err(error) => return Err(error),
                };
            source = Some((target.clone(), declarations));
        }
        inventory.push(target);
    }
    cancel.check()?;
    let (source, declarations) = source.ok_or(Error::Invalid("Source cloud is missing from its owning workspace"))?;
    Ok(Context {
        source,
        declarations,
        inventory,
    })
}

/// A local checkout of a declared companion repository, read to create its cloud.
#[derive(Clone, Debug)]
pub struct Checkout {
    pub repository: std::path::PathBuf,
    pub revision: String,
    pub profile: super::super::prices::Profile,
}

/// Reads `directory` as a checkout of `declaration`: its GitHub origin must be the
/// declared repository, and its committed `.horizon/cloud.yml` must define the
/// declared profile on a provider Horizon creates clouds on.
/// # Errors
/// Another repository, a missing or invalid configuration, or cancellation.
pub fn checkout(directory: &Path, declaration: &Declaration, cancel: &Cancellation) -> Result<Checkout> {
    let runner = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let prepared = repository::launch::prepare(&directory.to_string_lossy(), "HEAD", &runner)?;
    let found = Declaration::new(identity(&prepared.repository, &runner)?, declaration.profile.clone());
    if !found.matches(declaration) {
        return Err(Error::Invalid("This checkout is not the companion's repository"));
    }
    let profile = prepared
        .config
        .profiles
        .get(&declaration.profile)
        .cloned()
        .ok_or(Error::Invalid(
            "The checkout's committed .horizon/cloud.yml has no creatable profile with the companion's profile name",
        ))?;
    Ok(Checkout {
        repository: prepared.repository,
        revision: prepared.revision,
        profile,
    })
}

/// The GitHub `owner/name` of a checkout's origin. The origin URL is never emitted, since
/// it may embed a credential that is refused only after it is read.
/// # Errors
/// The checkout has no origin, or one that is not a credential-free GitHub URL.
pub(in crate::cloud_runtime) fn identity(path: &Path, runner: &Runner<'_>) -> Result<String> {
    let remote = Runner {
        cancel: runner.cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    }
    .run(
        "Read companion repository identity",
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["remote", "get-url", "origin"]),
        Duration::from_secs(10),
    )?;
    from_remote(remote.trim())
}

fn from_remote(remote: &str) -> Result<String> {
    let name = ["https://github.com/", "ssh://git@github.com/", "git@github.com:"]
        .into_iter()
        .find_map(|prefix| remote.strip_prefix(prefix))
        .ok_or(Error::Invalid(
            "Companions require a GitHub origin without embedded credentials",
        ))?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    Declaration::new(name, "identity")
        .validate()
        .map_err(|_| Error::Invalid("Invalid companion repository origin"))?;
    Ok(name.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_rejects_cancellation_and_duplicate_ids_despite_failed_repository_probes() {
        let cancel = Cancellation::default();
        cancel.cancel();
        let owner = Owner {
            scope: horizon_cloud::companions::Scope {
                session_id: "session".into(),
                workspace_id: "workspace".into(),
            },
            cloud_id: "source".into(),
        };
        let mut group =
            crate::cloud_panel::CloudGroup::new(1, "Target".into(), "workspace".into(), "/missing".into(), [0.0, 0.0]);
        group.remote = Some(
            serde_json::from_value(serde_json::json!({
                "id": "target", "revision": "a".repeat(40), "profile_name": "cpu",
                "profile": {"provider": "runpod", "image": "example/worker", "cpu": 8, "memory_gb": 32}
            }))
            .unwrap(),
        );
        let groups = CloudGroups(vec![group.clone(), group]);
        assert!(matches!(
            prepare(&owner, &groups, &cancel),
            Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
        ));
        assert!(matches!(
            prepare(&owner, &groups, &Cancellation::default()),
            Err(Error::Invalid("Cloud identity is ambiguous"))
        ));
    }

    #[test]
    fn a_source_without_committed_settings_declares_no_companions() {
        let temp = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(temp.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap()
        };
        git(&["init", "--quiet"]);
        git(&["remote", "add", "origin", "https://github.com/example/app.git"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "Application without cloud settings",
        ]);
        let revision = git(&["rev-parse", "HEAD"]).trim().to_owned();
        let owner = Owner {
            scope: horizon_cloud::companions::Scope {
                session_id: "session".into(),
                workspace_id: "workspace".into(),
            },
            cloud_id: "source".into(),
        };
        let mut group = crate::cloud_panel::CloudGroup::new(
            1,
            "Quick start".into(),
            "workspace".into(),
            temp.path().into(),
            [0.0, 0.0],
        );
        group.remote = Some(
            serde_json::from_value(serde_json::json!({
                "id": "source", "revision": revision, "profile_name": "quick-start",
                "profile": {"provider": "runpod", "image": "example/worker", "cpu": 2, "memory_gb": 4}
            }))
            .unwrap(),
        );
        let context = prepare(&owner, &CloudGroups(vec![group]), &Cancellation::default()).unwrap();
        assert!(context.declarations.is_empty());
        assert_eq!(context.source.cloud_id, "source");
    }

    #[test]
    fn a_checkout_must_be_the_declared_repository_with_the_declared_profile() {
        let temp = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(temp.path())
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        git(&["init", "--quiet"]);
        git(&["remote", "add", "origin", "https://github.com/example/consumer.git"]);
        std::fs::create_dir_all(temp.path().join(".horizon")).unwrap();
        std::fs::write(
            temp.path().join(".horizon/cloud.yml"),
            "version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n",
        )
        .unwrap();
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "Fixture",
        ]);
        let cancel = Cancellation::default();
        // A directory inside the checkout resolves to its root.
        let found = checkout(
            &temp.path().join(".horizon"),
            &Declaration::new("example/consumer", "cpu"),
            &cancel,
        )
        .unwrap();
        assert_eq!(found.repository, temp.path().canonicalize().unwrap());
        assert_eq!(found.revision.len(), 40);
        assert_eq!(found.profile.cpu, 4);
        for declaration in [
            Declaration::new("example/other", "cpu"),
            Declaration::new("example/consumer", "gpu"),
        ] {
            assert!(checkout(temp.path(), &declaration, &cancel).is_err());
        }
    }

    #[test]
    fn identity_accepts_supported_transports_and_rejects_credentials_and_paths() {
        for remote in [
            "https://github.com/example/app.git",
            "git@github.com:example/app.git",
            "ssh://git@github.com/example/app",
        ] {
            assert_eq!(from_remote(remote).unwrap(), "example/app");
        }
        for remote in [
            "/home/app",
            "https://token@github.com/example/app",
            "https://github.com/../app",
            "https://example.com/a/b",
        ] {
            assert!(from_remote(remote).is_err());
        }
    }
}

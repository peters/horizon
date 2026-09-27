//! Read committed declarations and local repository identity off the UI thread.
use super::super::{Cancellation, command::Runner, repository};
use super::{Context, Declaration, Error, Owner, Result, Target};
use crate::cloud_panel::CloudGroups;
use std::{collections::BTreeSet, path::Path, process::Command, time::Duration};

/// # Errors
/// The source must remain in the current owning workspace and have committed configuration.
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
            let prepared = repository::launch::prepare(&group.cwd.to_string_lossy(), &launch.revision, &runner)?;
            let declarations = prepared
                .config
                .cloud_companions()
                .map(|(alias, declaration)| (alias.to_owned(), declaration.clone()))
                .collect();
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

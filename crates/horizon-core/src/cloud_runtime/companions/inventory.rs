//! Read committed declarations and local repository identity off the UI thread.
use super::super::{Cancellation, command::Runner, repository};
use super::{Context, Declaration, Error, Owner, Result, Target};
use crate::cloud_panel::CloudGroups;
use std::{path::Path, process::Command, time::Duration};

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
    for group in &groups.0 {
        if group.workspace != owner.scope.workspace_id {
            continue;
        }
        let Some(launch) = &group.remote else { continue };
        let identity = identity(&group.cwd, &runner);
        if launch.id == owner.cloud_id {
            if source.is_some() {
                return Err(Error::Invalid("Source cloud identity is ambiguous"));
            }
            let prepared = repository::launch::prepare(&group.cwd.to_string_lossy(), &launch.revision, &runner)?;
            let target = Target {
                scope: owner.scope.clone(),
                cloud_id: launch.id.clone(),
                declaration: Declaration {
                    repository: identity?,
                    profile: launch.profile_name.clone(),
                },
            };
            source = Some((target.clone(), prepared.config.companions));
            inventory.push(target);
        } else if let Ok(repository) = identity {
            inventory.push(Target {
                scope: owner.scope.clone(),
                cloud_id: launch.id.clone(),
                declaration: Declaration {
                    repository,
                    profile: launch.profile_name.clone(),
                },
            });
        }
    }
    cancel.check()?;
    let (source, declarations) = source.ok_or(Error::Invalid("Source cloud is missing from its owning workspace"))?;
    Ok(Context {
        source,
        declarations,
        inventory,
    })
}

fn identity(path: &Path, runner: &Runner<'_>) -> Result<String> {
    let remote = runner.run(
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
    let declaration = Declaration {
        repository: name.into(),
        profile: "identity".into(),
    };
    declaration
        .validate()
        .map_err(|_| Error::Invalid("Invalid companion repository origin"))?;
    Ok(name.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_inventory_never_returns_a_partial_or_missing_source_result() {
        let cancel = Cancellation::default();
        cancel.cancel();
        let owner = Owner {
            scope: horizon_cloud::companions::Scope {
                session_id: "session".into(),
                workspace_id: "workspace".into(),
            },
            cloud_id: "source".into(),
        };
        assert!(matches!(
            prepare(&owner, &CloudGroups::default(), &cancel),
            Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
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

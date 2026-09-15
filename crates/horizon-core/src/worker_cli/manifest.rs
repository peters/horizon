//! Repository-owned environments, independent of cloud account settings.

use super::Error;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, io::Read, path::Path};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_environment: Option<String>,
    environments: BTreeMap<String, Environment>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Environment {
    image: String,
    directory: String,
    #[serde(default)]
    checks: BTreeMap<String, Vec<String>>,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}

fn valid_environment(environment: &Environment) -> bool {
    // Reuse the nonexecuting workspace validator for image and directory grammar.
    // Provider admission still uses the caller's actual profile at task creation.
    let spec = serde_json::from_value::<horizon_core::remote_workspace::RemoteWorkspaceSpec>(serde_json::json!({
        "workspace_local_id": "manifest-validation",
        "target": {"provider": "local_docker", "profile": "manifest-validation",
            "image": environment.image, "disk_gib": 20, "lifetime": "persistent"},
        "repository": {"repository": "fixture/repository", "commit": "a".repeat(40), "branch": null},
        "working_directory": environment.directory, "generation": 0, "panels": []
    }));
    spec.is_ok_and(|spec| spec.validate().is_ok())
        && environment.checks.len() <= 16
        && environment.checks.iter().all(|(name, argv)| {
            valid_name(name)
                && !argv.is_empty()
                && !argv[0].is_empty()
                && argv.len() <= 64
                && argv.iter().all(|arg| arg.len() <= 8192 && !arg.contains('\0'))
        })
}

pub(super) fn parse(text: &str) -> Result<Value, Error> {
    let manifest: Manifest = serde_yaml::from_str(text).map_err(|_| Error::Input)?;
    if manifest.version != 1
        || manifest.environments.is_empty()
        || manifest.environments.len() > 32
        || manifest
            .default_environment
            .as_ref()
            .is_some_and(|name| !manifest.environments.contains_key(name))
        || !manifest
            .environments
            .iter()
            .all(|(name, environment)| valid_name(name) && valid_environment(environment))
    {
        return Err(Error::Input);
    }
    serde_json::to_value(manifest).map_err(|_| Error::Input)
}

pub(super) fn read(path: &Path) -> Result<Value, Error> {
    let mut text = String::new();
    std::fs::File::open(path)
        .map_err(|_| Error::Input)?
        .take(65_537)
        .read_to_string(&mut text)
        .map_err(|_| Error::Input)?;
    if text.len() > 65_536 {
        return Err(Error::Input);
    }
    parse(&text)
}

#[cfg(test)]
mod tests {
    use super::parse;

    fn fixture() -> String {
        format!(
            "version: 1\ndefault_environment: web\nenvironments:\n  web:\n    image: registry.example/web@sha256:{}\n    directory: apps/web\n    checks:\n      test: [npm, test]\n  api:\n    image: registry.example/api@sha256:{}\n    directory: services/api\n",
            "a".repeat(64),
            "b".repeat(64)
        )
    }

    #[test]
    fn accepts_monorepo_images_and_argv_without_execution() {
        let parsed = parse(&fixture()).unwrap();
        assert_eq!(parsed["environments"]["web"]["checks"]["test"][0], "npm");
        assert_eq!(parsed["environments"].as_object().unwrap().len(), 2);
        assert_ne!(
            parsed["environments"]["web"]["image"],
            parsed["environments"]["api"]["image"]
        );
    }

    #[test]
    fn rejects_mutable_images_missing_defaults_and_escaping_directories() {
        for broken in [
            fixture().replace(&format!("@sha256:{}", "a".repeat(64)), ":latest"),
            fixture().replace("default_environment: web", "default_environment: absent"),
            fixture().replace("apps/web", "../web"),
            fixture().replace("apps/web", "/web"),
            fixture().replace("apps/web", "a/."),
            fixture().replace("apps/web", "a:b"),
            fixture().replace("apps/web", "\"a\\tb\""),
            fixture().replace("registry.example/web", "registry.example//web"),
            fixture() + "subscription: private-account\n",
        ] {
            assert!(parse(&broken).is_err());
        }
    }
}

use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn isolated_registry_config_preserves_the_daemon_context_without_other_credentials() {
    const CHILD: &str = "HORIZON_REGISTRY_CONFIG_FIXTURE";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = std::path::PathBuf::from(root);
        check_private_configs(&root);
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let machine = root.path().join("machine");
    std::fs::create_dir(&machine).unwrap();
    for folder in ["contexts", "cli-plugins"] {
        std::fs::create_dir(machine.join(folder)).unwrap();
    }
    let original = serde_json::to_vec(&serde_json::json!({
        "currentContext":"fixture-rootless","cliPluginsExtraDirs":["/fixture/plugins"],
        "auths":{"another.example":{"auth":"synthetic-machine-login"}},
        "credsStore":"fixture-helper","credHelpers":{"another.example":"fixture-helper"},
        "proxies":{"default":{"httpProxy":"http://fixture:synthetic@proxy.example"}}
    }))
    .unwrap();
    std::fs::write(machine.join("config.json"), &original).unwrap();
    std::fs::write(root.path().join("grant"), "synthetic-dedicated-grant").unwrap();
    std::fs::set_permissions(root.path().join("grant"), std::fs::Permissions::from_mode(0o600)).unwrap();
    let tools = root.path().join("tools");
    std::fs::create_dir(&tools).unwrap();
    let docker = tools.join("docker");
    std::fs::write(
        &docker,
        r"#!/usr/bin/env python3
import json, os, pathlib, sys
config_root = pathlib.Path(sys.argv[sys.argv.index('--config') + 1])
config = json.loads((config_root / 'config.json').read_text())
assert config['currentContext'] == 'fixture-rootless'
assert config['cliPluginsExtraDirs'] == ['/fixture/plugins']
assert config['auths'] == {'registry.example': {}}
assert set(config) <= {'auths', 'currentContext', 'cliPluginsExtraDirs'}
assert (config_root / 'contexts').is_dir()
assert (config_root / 'cli-plugins').is_dir()
assert config_root.stat().st_mode & 0o777 == 0o700
assert (config_root / 'config.json').stat().st_mode & 0o777 == 0o600
assert 'login' in sys.argv and '--password-stdin' in sys.argv
username = sys.argv[sys.argv.index('--username') + 1]
assert sys.stdin.read() == {'reader':'synthetic-pull', 'writer':'synthetic-push'}[username]
with (pathlib.Path(os.environ['HORIZON_REGISTRY_CONFIG_FIXTURE']) / 'logins').open('a') as log:
    log.write(username + '\n')
",
    )
    .unwrap();
    std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&machine, root.path().join(".docker")).unwrap();
    for configuration in [
        Some(std::path::Path::new("machine")),
        Some(machine.as_path()),
        Some(std::path::Path::new("")),
        None,
    ] {
        let _ = std::fs::remove_file(root.path().join("logins"));
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
        .current_dir(root.path())
        .args([
            "cloud_runtime::registry::tests::docker_context::isolated_registry_config_preserves_the_daemon_context_without_other_credentials",
            "--exact",
        ])
        .env(CHILD, root.path())
        .env("HOME", root.path())
        .env("PATH", std::env::join_paths(std::iter::once(tools.clone()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))).unwrap())
;
        match configuration {
            Some(path) => {
                command.env("DOCKER_CONFIG", path);
            }
            None => {
                command.env_remove("DOCKER_CONFIG");
            }
        }
        let status = command.status().unwrap();
        assert!(status.success());
    }
    assert_eq!(std::fs::read(machine.join("config.json")).unwrap(), original);
}

fn check_private_configs(root: &std::path::Path) {
    let material = Material::load(
        &Auth {
            username: "fixture-writer".into(),
            secret_file: root.join("grant"),
            expires_at: None,
        },
        "registry.example/team/worker",
        None,
    )
    .unwrap();
    let path = material.config.path();
    let config: serde_json::Value = serde_json::from_slice(&std::fs::read(path.join("config.json")).unwrap()).unwrap();
    assert_eq!(config["currentContext"], "fixture-rootless");
    assert_eq!(config["cliPluginsExtraDirs"], serde_json::json!(["/fixture/plugins"]));
    assert_eq!(config["auths"].as_object().unwrap().len(), 1);
    assert!(config["auths"]["registry.example"]["auth"].is_string());
    for field in ["credsStore", "credHelpers", "proxies"] {
        assert!(config.get(field).is_none(), "machine credential settings must stay out");
    }
    assert!(path.join("contexts").is_dir());
    assert!(path.join("cli-plugins").is_dir());
    assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o700);
    let (_fixture, settings) = fixture();
    let prepared = Prepared::for_image_for_provider(
        &settings,
        "registry.example/team/worker",
        None,
        true,
        horizon_cloud::provider::Kind::DockerHost,
    )
    .unwrap()
    .unwrap();
    let cancel = Cancellation::default();
    let runner = crate::cloud_runtime::command::Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    prepared.preflight(&runner).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("logins")).unwrap(),
        "reader\nwriter\n"
    );
}

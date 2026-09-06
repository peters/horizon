use super::*;
use crate::Config;

const EXPLICIT_PROFILES: &str = "remote:
  local_docker:
    - name: development
      docker_host: unix:///explicit path/not-connected.sock
    - name: windows
      docker_host: npipe:////./pipe/explicit-daemon
";

#[test]
fn old_and_default_configurations_remain_empty_without_a_migration() {
    for configuration in [
        Config::default(),
        Config::from_yaml("{}").expect("minimal"),
        Config::from_yaml("version: 1\npresets: []\n").expect("legacy"),
        Config::from_yaml("remote: {}\n").expect("empty remote"),
        Config::from_yaml("remote:\n  local_docker: []\n").expect("empty profiles"),
    ] {
        assert!(configuration.remote.is_empty());
        let yaml = configuration.to_yaml().expect("serialize");
        assert!(!yaml.contains("\nremote:"));
        let restored = Config::from_yaml(&yaml).expect("round trip");
        assert!(restored.remote.is_empty());
        assert_eq!(restored.version, configuration.version);
    }
}

#[test]
fn explicit_profiles_round_trip_through_the_full_configuration() {
    let configuration = Config::from_yaml(EXPLICIT_PROFILES).expect("configuration");
    let expected = super::config(vec![
        profile("development", "unix:///explicit path/not-connected.sock"),
        profile("windows", "npipe:////./pipe/explicit-daemon"),
    ]);
    assert_eq!(configuration.remote, expected);
    let yaml = configuration.to_yaml().expect("serialize");
    assert_eq!(Config::from_yaml(&yaml).expect("round trip").remote, expected);
    assert_eq!(
        configuration.remote.local_docker_profile("development"),
        Ok(&expected.local_docker[0])
    );
}

#[test]
fn full_configuration_semantically_validates_profiles() {
    for remote in [
        "local_docker: [{name: ' local', docker_host: 'unix:///explicit.sock'}]",
        "local_docker: [{name: local, docker_host: 'tcp://127.0.0.1:2375'}]",
        "local_docker: [{name: local, docker_host: 'unix:///one.sock'}, {name: local, docker_host: 'unix:///two.sock'}]",
    ] {
        assert!(Config::from_yaml(&format!("remote:\n  {remote}\n")).is_err());
    }
    let mut edited = Config::default();
    edited.remote.local_docker.push(profile("", "unix:///explicit.sock"));
    assert!(edited.validate().is_err());
}

#[test]
fn full_configuration_redacts_malformed_profile_values() {
    for remote in [
        "synthetic-private-marker",
        "{local_docker: synthetic-private-marker}",
        "{local_docker: [synthetic-private-marker]}",
        "{local_docker: [{name: local, docker_host: [synthetic-private-marker]}]}",
        "{synthetic-private-marker: true}",
        "{local_docker: [{name: synthetic-private-marker, docker_host: 'ssh://synthetic-private-marker'}]}",
    ] {
        let error = Config::from_yaml(&format!("remote: {remote}\n")).expect_err("invalid remote configuration");
        assert!(!format!("{error:?}: {error}").contains("synthetic-private-marker"));
    }
}

#[test]
fn explicit_profile_load_does_not_connect_or_rewrite_current_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("config.yaml");
    let yaml = format!("version: {}\n{EXPLICIT_PROFILES}", Config::default().version);
    std::fs::write(&path, &yaml).expect("fixture");
    let loaded = Config::load(Some(&path)).expect("load without daemon");
    assert_eq!(loaded.remote.local_docker.len(), 2);
    assert_eq!(std::fs::read_to_string(&path).expect("unchanged config"), yaml);
    assert_eq!(std::fs::read_dir(temp.path()).expect("directory").count(), 1);
}

#[test]
fn existing_config_migration_preserves_explicit_remote_profiles() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("config.yaml");
    let original = format!("version: 1\n{EXPLICIT_PROFILES}");
    std::fs::write(&path, &original).expect("legacy fixture");
    let before = Config::from_yaml(&original).expect("legacy parse");
    let migrated = Config::load(Some(&path)).expect("migrate");
    assert_eq!(migrated.version, Config::default().version);
    assert_eq!(migrated.remote, before.remote);
    let saved = std::fs::read_to_string(&path).expect("migrated config");
    assert_eq!(Config::from_yaml(&saved).expect("reopen").remote, before.remote);
}

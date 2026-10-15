use super::*;
use horizon_core::{PanelKind, PanelOptions, browser_actor, cloud_runtime::setup};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

struct Fixture {
    _temp: tempfile::TempDir,
    app: HorizonApp,
    actor: String,
    panel: horizon_core::PanelId,
    source: PathBuf,
    root: PathBuf,
}
fn fixture() -> Fixture {
    let (temp, mut app) = crate::app::test_support::test_app();
    let source = temp.path().join("source");
    let root = temp.path().join("cloud");
    std::fs::create_dir_all(source.join(".horizon")).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(source.join(".horizon/cloud.yml"),"version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: runpod\n    cpu: 2\n    memory_gb: 4\n    build:\n      context: .\n      dockerfile: Dockerfile\n").unwrap();
    std::fs::write(source.join("Dockerfile"), "FROM scratch\n").unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
        vec!["remote", "add", "origin", "https://gitlab.example/group/source.git"],
    ] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    let mut settings = setup::load_settings(&root).unwrap();
    settings.portable_image_publishing = true;
    settings.registries=Some(serde_json::from_value(json!({"root":root.join("registry"),"bindings":[{"repository":"registry.example/group/worker","generation":"synthetic-generation","publish":{"username":"builder","secret_file":root.join("absent-publish"),"expires_at":null},"pull":{"username":"reader","secret_file":root.join("absent-pull"),"expires_at":null},"read_only_confirmed":true,"retired":[]}]})).unwrap());
    std::fs::write(root.join("settings.json"), serde_json::to_vec(&settings).unwrap()).unwrap();
    let workspace = app.board.create_workspace("image controls");
    let panel = app
        .board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Codex,
                cwd: Some(source.clone()),
                command: Some("/bin/sh".into()),
                args: vec!["-c".into(), "exit 0".into()],
                ..PanelOptions::default()
            },
            workspace,
        )
        .unwrap();
    let actor = browser_actor(&app.board.panel(panel).unwrap().local_id);
    app.cloud_prototype.root = Some(root.clone());
    Fixture {
        _temp: temp,
        app,
        actor,
        panel,
        source,
        root,
    }
}
fn request(actor: &str, id: &str, profile: Option<&str>, action: Value) -> UsageRequest {
    let mut request = json!({"request_id":manifest::provider_usage::new_operation_id(),"actor":actor,"host_instance":manifest::host_instance(),"deadline_at_millis":manifest::now_millis()+15_000,"claimed":true,"image_destination":{"operation_id":id,"profile":profile,"action":null}});
    request["image_destination"]["action"] = action;
    serde_json::from_value(request).unwrap()
}
fn run(f: &mut Fixture, action: Value) -> Value {
    run_profile(f, "cpu", action)
}

fn run_profile(f: &mut Fixture, profile: &str, action: Value) -> Value {
    let id = manifest::provider_usage::new_operation_id();
    let ctx = egui::Context::default();
    let started = f
        .app
        .image_destination_result(&request(&f.actor, &id, Some(profile), action), &ctx);
    assert!(started.error.is_none(), "{:?}", started.error);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let result = f
            .app
            .image_destination_result(&request(&f.actor, &id, None, json!({"operation":"status"})), &ctx);
        assert!(result.error.is_none(), "{:?}", result.error);
        let status = result.image_destination.unwrap();
        if status["done"] == true {
            assert!(status["error"].is_null(), "{status}");
            return status["result"].clone();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
#[cfg_attr(
    windows,
    ignore = "Local agent fixture uses a POSIX shell and durable Unix cloud settings"
)]
fn local_actor_changes_only_its_registered_checkout_without_settings_or_secrets() {
    let mut f = fixture();
    let original = std::fs::read(f.source.join(".horizon/cloud.yml")).unwrap();
    let info = run(&mut f, json!({"operation":"list"}));
    assert!(info["shown_override"].is_null());
    let selected = run(
        &mut f,
        json!({"operation":"select","shown":null,"destination":info["choices"][0]["selection"]}),
    );
    assert_eq!(
        selected["shown_override"]["repository"],
        json!(f.source.canonicalize().unwrap())
    );
    assert!(f.app.settings.is_none());
    let reset = run(&mut f, json!({"operation":"reset","shown":selected["shown_override"]}));
    assert!(reset["shown_override"].is_null());
    assert_eq!(std::fs::read(f.source.join(".horizon/cloud.yml")).unwrap(), original);
    assert!(!f.root.join("registry").exists());
    assert!(!f.root.join("docker").exists());
    assert!(!serde_json::to_string(&selected).unwrap().contains("absent-publish"));
}
#[test]
#[cfg_attr(windows, ignore = "Local agent fixture uses a POSIX shell")]
fn expired_foreign_and_remote_scope_requests_cannot_start_work() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    let id = manifest::provider_usage::new_operation_id();
    let mut input = request(&f.actor, &id, Some("cpu"), json!({"operation":"list"}));
    input.deadline_at_millis = 0;
    assert!(f.app.image_destination_result(&input, &ctx).error.is_some());
    input.deadline_at_millis = manifest::now_millis() + 15_000;
    input.actor = "horizon:foreign".into();
    assert!(f.app.image_destination_result(&input, &ctx).error.is_some());
    input.actor = f.actor.clone();
    input.host_instance = "another-host".into();
    assert!(f.app.image_destination_result(&input, &ctx).error.is_some());
    input.host_instance = manifest::host_instance().into();
    f.app.board.panel_mut(f.panel).unwrap().ssh_connection = Some(horizon_core::SshConnection {
        host: "worker.example".into(),
        ..horizon_core::SshConnection::default()
    });
    assert!(f.app.image_destination_result(&input, &ctx).error.is_some());
    f.app.board.panel_mut(f.panel).unwrap().ssh_connection = None;
    f.app.board.panel_mut(f.panel).unwrap().launch_cwd = None;
    assert!(f.app.image_destination_result(&input, &ctx).error.is_some());
}

#[test]
#[cfg_attr(
    windows,
    ignore = "Local agent fixture uses a POSIX shell and durable Unix cloud settings"
)]
fn an_agent_started_in_a_subdirectory_manages_the_checkout_root() {
    let mut f = fixture();
    let subdirectory = f.source.join("nested").join("work");
    std::fs::create_dir_all(&subdirectory).unwrap();
    f.app.board.panel_mut(f.panel).unwrap().launch_cwd = Some(subdirectory);
    let original = std::fs::read(f.source.join(".horizon/cloud.yml")).unwrap();
    let info = run(&mut f, json!({"operation":"list"}));
    let selected = run(
        &mut f,
        json!({"operation":"select","shown":null,"destination":info["choices"][0]["selection"]}),
    );
    assert_eq!(
        selected["shown_override"]["repository"],
        json!(f.source.canonicalize().unwrap())
    );
    let reset = run(&mut f, json!({"operation":"reset","shown":selected["shown_override"]}));
    assert!(reset["shown_override"].is_null());
    assert_eq!(std::fs::read(f.source.join(".horizon/cloud.yml")).unwrap(), original);
    assert!(!f.root.join("registry").exists());
}

#[test]
#[cfg(unix)]
#[ignore = "set HORIZON_TEST_REPOSITORY to a real checkout with the cpu profile for a read-only host listing"]
fn real_repository_public_listing_from_a_registered_subdirectory_is_read_only() {
    let mut f = fixture();
    let source = PathBuf::from(std::env::var_os("HORIZON_TEST_REPOSITORY").unwrap());
    let subdirectory = source.join("crates/horizon-core");
    assert!(subdirectory.is_dir());
    let yaml = source.join(".horizon/cloud.yml");
    let before_source = std::fs::read(&yaml).unwrap();
    let before_settings = std::fs::read(f.root.join("settings.json")).unwrap();
    f.app.board.panel_mut(f.panel).unwrap().launch_cwd = Some(subdirectory);
    let info = run(&mut f, json!({"operation":"list"}));
    assert!(info["enabled"].as_bool().unwrap());
    assert_eq!(info["choices"].as_array().unwrap().len(), 1);
    assert_eq!(std::fs::read(yaml).unwrap(), before_source);
    assert_eq!(std::fs::read(f.root.join("settings.json")).unwrap(), before_settings);
    assert!(!f.root.join("registry").exists());
    assert!(!f.root.join("docker").exists());
}

#[test]
#[cfg_attr(
    windows,
    ignore = "Local agent fixture uses a POSIX shell and durable Unix cloud settings"
)]
fn public_actions_accept_the_shared_policy_profile_name_limit() {
    let mut f = fixture();
    let name = "p".repeat(100);
    let path = f.source.join(".horizon/cloud.yml");
    let yaml = std::fs::read_to_string(&path)
        .unwrap()
        .replace("default: cpu\n", &format!("default: {name}\n"))
        .replace("  cpu:\n", &format!("  {name}:\n"));
    std::fs::write(&path, &yaml).unwrap();
    let info = run_profile(&mut f, &name, json!({"operation":"list"}));
    let selected = run_profile(
        &mut f,
        &name,
        json!({"operation":"select","shown":null,"destination":info["choices"][0]["selection"]}),
    );
    assert_eq!(selected["shown_override"]["profile"], name);
    let reset = run_profile(
        &mut f,
        &name,
        json!({"operation":"reset","shown":selected["shown_override"]}),
    );
    assert!(reset["shown_override"].is_null());
    assert_eq!(std::fs::read_to_string(path).unwrap(), yaml);
}

#[test]
#[cfg_attr(windows, ignore = "Local agent fixture uses a POSIX shell")]
fn cloud_group_members_cannot_manage_desktop_destinations() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    let panel = f.app.board.panel(f.panel).unwrap();
    let mut group = horizon_core::cloud_panel::CloudGroup::new(
        1,
        "remote worker".into(),
        "workspace".into(),
        f.source.clone(),
        [0.0, 0.0],
    );
    group.panels.push(panel.local_id.clone());
    f.app.cloud_prototype.groups.0.push(group);
    let before = std::fs::read(f.root.join("settings.json")).unwrap();
    for action in [
        json!({"operation":"list"}),
        json!({"operation":"reset","shown":null}),
        json!({"operation":"authenticate","shown":null,"switch":false}),
    ] {
        let id = manifest::provider_usage::new_operation_id();
        let input = request(&f.actor, &id, Some("cpu"), action);
        let result = f.app.image_destination_result(&input, &ctx);
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|error| error.contains("local agent checkout")),
            "{:?}",
            result.error
        );
    }
    assert!(!f.app.browser_create_host.provider_usage.image_destinations.running());
    assert_eq!(std::fs::read(f.root.join("settings.json")).unwrap(), before);
    assert!(!f.root.join("github-publishing").exists());
}

#[test]
#[cfg_attr(windows, ignore = "Local agent fixture uses a POSIX shell")]
fn saved_cloud_membership_refuses_admission_before_live_groups_are_restored() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    let panel = f.app.board.panel(f.panel).unwrap();
    let mut group = horizon_core::cloud_panel::CloudGroup::new(
        1,
        "restored worker".into(),
        "workspace".into(),
        f.source.clone(),
        [0.0, 0.0],
    );
    group.panels.push(panel.local_id.clone());
    f.app.board.cloud_groups.0.push(group);
    assert!(f.app.cloud_prototype.groups.0.is_empty());
    let before = std::fs::read(f.root.join("settings.json")).unwrap();
    for operation in ["list", "reset", "authenticate"] {
        let id = manifest::provider_usage::new_operation_id();
        let action = match operation {
            "list" => json!({"operation":"list"}),
            "reset" => json!({"operation":"reset","shown":null}),
            _ => json!({"operation":"authenticate","shown":null,"switch":false}),
        };
        let input = request(&f.actor, &id, Some("cpu"), action);
        let result = f.app.image_destination_result(&input, &ctx);
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|error| error.contains("local agent checkout")),
            "saved cloud membership must refuse before work starts"
        );
    }
    assert!(!f.app.browser_create_host.provider_usage.image_destinations.running());
    assert_eq!(std::fs::read(f.root.join("settings.json")).unwrap(), before);
}

#[test]
#[cfg_attr(windows, ignore = "Local agent fixture uses a POSIX shell")]
fn restored_or_live_cloud_membership_cancels_retained_destination_operations() {
    use horizon_core::cloud_runtime::publishing::operations::Owner;
    for saved in [false, true] {
        let mut f = fixture();
        let ctx = egui::Context::default();
        let id = manifest::provider_usage::new_operation_id();
        let input = request(&f.actor, &id, Some("cpu"), json!({"operation":"list"}));
        let started = f.app.image_destination_result(&input, &ctx);
        assert!(started.error.is_none());
        assert_eq!(started.image_destination.unwrap()["done"], false);
        let mut group = horizon_core::cloud_panel::CloudGroup::new(
            1,
            "worker".into(),
            "workspace".into(),
            f.source.clone(),
            [0.0, 0.0],
        );
        group.panels.push(f.app.board.panel(f.panel).unwrap().local_id.clone());
        if saved {
            f.app.board.cloud_groups.0.push(group);
        } else {
            f.app.cloud_prototype.groups.0.push(group);
        }
        f.app.poll_image_destination_operations(&ctx);
        let owner = Owner {
            actor: f.actor.clone(),
            host: manifest::host_instance().into(),
            root: f.root.clone(),
            repository: f.source.clone(),
        };
        let manager = &mut f.app.browser_create_host.provider_usage.image_destinations;
        assert!(
            manager.status(&owner, &id).unwrap().cancel_requested,
            "membership must invalidate a retained owner before restoration copies it"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !manager.status(&owner, &id).unwrap().done {
            assert!(Instant::now() < deadline, "cancelled metadata operation must settle");
            std::thread::yield_now();
        }
    }
}

#[test]
#[cfg_attr(windows, ignore = "Local agent fixture uses a POSIX shell")]
fn destination_changes_preserve_saved_agent_bindings_without_reading_credentials() {
    let mut f = fixture();
    let path = f.root.join("settings.json");
    let token = json!({"file":f.root.join("absent-agent-subscription"),"saved_at":0});
    let mut settings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    settings["claude_token"] = token.clone();
    std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let yaml = std::fs::read(f.source.join(".horizon/cloud.yml")).unwrap();
    let info = run(&mut f, json!({"operation":"list"}));
    let selected = run(
        &mut f,
        json!({"operation":"select","shown":null,"destination":info["choices"][0]["selection"]}),
    );
    let after: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(after["claude_token"], token);
    assert!(
        !serde_json::to_string(&selected)
            .unwrap()
            .contains("absent-agent-subscription")
    );
    run(&mut f, json!({"operation":"reset","shown":selected["shown_override"]}));
    let reset: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(reset["claude_token"], token);
    assert!(!f.root.join("absent-agent-subscription").exists());
    assert_eq!(std::fs::read(f.source.join(".horizon/cloud.yml")).unwrap(), yaml);
}

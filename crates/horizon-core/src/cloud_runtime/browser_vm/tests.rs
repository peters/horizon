//! The record of the browser VM and the refusals that keep it honest. The rental
//! sequence itself is tested against a scripted provider in `horizon_cloud::browser_vm`.
use super::*;
use std::os::unix::fs::PermissionsExt;

const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f";
const IMAGE: &str =
    "ghcr.io/example/browser-vm@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn private_file(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// A cloud settings directory of this machine: a token, an identity pair and the
/// Hetzner section, with `locations` allowed.
fn root(locations: &[&str]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let identity = root.path().join("identity");
    private_file(&identity, "synthetic private key");
    private_file(&root.path().join("identity.pub"), KEY);
    private_file(&root.path().join("token"), "synthetic-hetzner-token");
    let settings = serde_json::json!({
        "runpod_key_file": "/unused", "ssh_identity_file": identity, "docker_config": "/unused",
        "cpu_flavors": [], "gpu_types": [],
        "hetzner": {"token_file": root.path().join("token"), "server_types": ["cx43"], "locations": locations},
    });
    std::fs::write(root.path().join("settings.json"), settings.to_string()).unwrap();
    root
}

fn deploy() -> Deploy {
    Deploy {
        image: IMAGE.into(),
        server_type: "ccx13".into(),
        location: "fsn1".into(),
        volume_gb: DEFAULT_VOLUME_GB,
    }
}

/// The refusal of a deploy that never reaches the provider.
fn refused(root: &Path, deploy: &Deploy) -> String {
    let error = super::begin(root, &Operation::Deploy(deploy.clone())).unwrap_err();
    assert!(
        !root.join(RECORD).exists(),
        "a refused request claims no run and leaves no record"
    );
    error.to_string()
}

#[test]
fn a_mutable_image_tag_is_refused_before_a_machine_is_rented() {
    let root = root(&["fsn1"]);
    let mut tagged = deploy();
    tagged.image = "ghcr.io/example/browser-vm:1780".into();
    let error = refused(root.path(), &tagged);
    assert!(error.contains("digest"), "{error}");
}

#[test]
fn a_shared_core_server_type_is_refused_with_the_reason() {
    let root = root(&["fsn1"]);
    let mut shared = deploy();
    shared.server_type = "cx43".into();
    let error = refused(root.path(), &shared);
    assert!(error.contains("ccx"), "{error}");
}

#[test]
fn a_location_the_hetzner_settings_do_not_allow_is_refused() {
    let root = root(&["hel1"]);
    let error = refused(root.path(), &deploy());
    assert!(error.contains("location"), "{error}");
}

#[test]
fn a_state_volume_outside_the_provider_limits_is_refused() {
    let root = root(&["fsn1"]);
    let mut small = deploy();
    small.volume_gb = 5;
    assert!(refused(root.path(), &small).contains("volume"));
}

#[test]
fn a_horizon_without_a_hetzner_section_is_refused() {
    let root = tempfile::tempdir().unwrap();
    private_file(&root.path().join("identity"), "synthetic");
    private_file(&root.path().join("identity.pub"), KEY);
    let settings = serde_json::json!({
        "runpod_key_file": "/unused", "ssh_identity_file": root.path().join("identity"),
        "docker_config": "/unused", "cpu_flavors": [], "gpu_types": [],
    });
    std::fs::write(root.path().join("settings.json"), settings.to_string()).unwrap();
    assert!(refused(root.path(), &deploy()).contains("hetzner"));
}

#[test]
fn a_fresh_horizon_holds_no_browser_vm() {
    let root = root(&["fsn1"]);
    let status = super::status(root.path()).unwrap();
    assert_eq!(status.phase, Phase::Idle);
    assert!(status.ssh_target.is_none() && status.volume_gb.is_none());
    assert!(status.phase.idle());
}

#[test]
fn a_record_of_another_shape_is_reported_rather_than_started_afresh() {
    let root = root(&["fsn1"]);
    std::fs::write(root.path().join(RECORD), br#"{"phase":"ready","novel":1}"#).unwrap();
    assert!(super::status(root.path()).is_err());
}

#[test]
fn the_record_keeps_the_machine_and_its_volume_across_a_round_trip() {
    let root = root(&["fsn1"]);
    let record = Record {
        phase: Phase::Stopped,
        image: Some(IMAGE.into()),
        server_type: Some("ccx13".into()),
        location: Some("fsn1".into()),
        volume_gb: Some(DEFAULT_VOLUME_GB),
        operation: CreateState::Bound { worker_id: "42".into() },
        journal: Journal {
            location: Some("fsn1".into()),
            volume: CreateState::Bound { worker_id: "9".into() },
            released: Some("42".into()),
            ..Journal::default()
        },
        ..Record::default()
    };
    record.save(root.path()).unwrap();
    assert_eq!(Record::load(root.path()).unwrap(), record);
    // A stopped machine is resumed, not deployed again, so its sign-ins stay.
    let error = super::begin(root.path(), &Operation::Deploy(deploy())).unwrap_err();
    assert!(error.to_string().contains("resume"), "{error}");
    assert_eq!(super::status(root.path()).unwrap().phase, Phase::Stopped);
}

#[test]
fn only_a_finished_stop_is_resumed_and_it_keeps_its_volume() {
    let root = root(&["fsn1"]);
    let ready = Record {
        phase: Phase::Ready,
        image: Some(IMAGE.into()),
        server_type: Some("ccx13".into()),
        location: Some("fsn1".into()),
        volume_gb: Some(DEFAULT_VOLUME_GB),
        operation: CreateState::Bound { worker_id: "42".into() },
        journal: Journal {
            location: Some("fsn1".into()),
            volume: CreateState::Bound { worker_id: "9".into() },
            ..Journal::default()
        },
        ..Record::default()
    };
    ready.save(root.path()).unwrap();
    let refused = super::begin(root.path(), &Operation::Resume).unwrap_err();
    assert!(refused.to_string().contains("Stop"), "{refused}");

    // A stop released the server and kept the volume, so the resume clears the fence.
    let stopped = Record {
        phase: Phase::Stopped,
        operation: CreateState::Bound { worker_id: "42".into() },
        journal: Journal {
            released: Some("42".into()),
            ..ready.journal.clone()
        },
        ..ready
    };
    stopped.save(root.path()).unwrap();
    let begun = super::begin(root.path(), &Operation::Resume).unwrap();
    assert_eq!(begun.phase, Phase::Resuming);
    let recorded = Record::load(root.path()).unwrap();
    assert_eq!(recorded.operation, CreateState::Prepared);
    assert!(recorded.journal.released.is_none() && !recorded.fresh);
    assert_eq!(
        recorded.journal.volume,
        CreateState::Bound { worker_id: "9".into() },
        "the sign-ins stay on the volume"
    );
}

#[test]
fn a_deploy_records_the_phase_and_whether_it_may_start_afresh() {
    let root = root(&["fsn1"]);
    let begun = super::begin(root.path(), &Operation::Deploy(deploy())).unwrap();
    assert_eq!(begun.phase, Phase::Deploying);
    let recorded = Record::load(root.path()).unwrap();
    assert!(recorded.fresh, "nothing was rented before");
    assert_eq!(recorded.image.as_deref(), Some(IMAGE));
    // The machine is now in a run, so nothing else starts until it ends.
    assert!(super::begin(root.path(), &Operation::Deploy(deploy())).is_err());
}

#[test]
fn a_run_in_progress_refuses_another_and_a_delete_with_nothing_refuses_too() {
    let root = root(&["fsn1"]);
    for phase in [Phase::Deploying, Phase::Stopping, Phase::Resuming, Phase::Deleting] {
        let record = Record {
            phase,
            ..Record::default()
        };
        record.save(root.path()).unwrap();
        assert!(!phase.idle());
        let busy = super::begin(root.path(), &Operation::Delete).unwrap_err();
        assert!(busy.to_string().contains("in progress"), "{busy}");
    }
    Record::default().save(root.path()).unwrap();
    let nothing = super::begin(root.path(), &Operation::Delete).unwrap_err();
    assert!(nothing.to_string().contains("nothing to delete"), "{nothing}");
    let stop = super::begin(root.path(), &Operation::Stop).unwrap_err();
    assert!(stop.to_string().contains("nothing to stop"), "{stop}");
    // A run with nothing recorded is refused rather than renting something.
    assert!(super::run(root.path(), &Cancellation::default(), &mut |_| {}).is_err());
}

#[test]
fn the_ssh_target_names_the_person_who_signs_in() {
    let status = Status {
        phase: Phase::Ready,
        ssh_target: Some("root@203.0.113.10".into()),
        image: None,
        server_type: None,
        location: None,
        volume_gb: None,
        hourly_net: None,
        error: None,
    };
    assert_eq!(status.ssh_host(), Some("203.0.113.10"));
    // A Device panel reaches the display on the loopback of that host.
    assert_eq!(VNC_ADDRESS, "127.0.0.1:5900");
}

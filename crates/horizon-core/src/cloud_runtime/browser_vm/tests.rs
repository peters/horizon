//! The record of the browser VM and the refusals that keep it honest. The rental
//! sequence itself is tested against a scripted provider in `horizon_cloud::browser_vm`.
use super::*;
use std::os::unix::fs::PermissionsExt;

const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f";
/// The identity that the cloud settings name, as the ssh entry writes it.
const IDENTITY: &str = "/home/person/.horizon/cloud/identity";
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
    assert!(recorded.journal.released.is_none());
    assert_eq!(
        recorded.journal.volume,
        CreateState::Bound { worker_id: "9".into() },
        "the sign-ins stay on the volume"
    );
}

#[test]
fn a_deploy_records_the_phase_and_the_machine_it_asks_for() {
    let root = root(&["fsn1"]);
    let begun = super::begin(root.path(), &Operation::Deploy(deploy())).unwrap();
    assert_eq!(begun.phase, Phase::Deploying);
    let recorded = Record::load(root.path()).unwrap();
    assert_eq!(recorded.image.as_deref(), Some(IMAGE));
    assert_eq!(recorded.operation, CreateState::Prepared, "nothing was rented before");
    // The machine is now in a run, and an explicit action takes it over.
    assert!(super::begin(root.path(), &Operation::Deploy(deploy())).is_ok());
}

#[test]
fn a_deploy_after_a_finished_delete_leaves_the_fences_that_a_rental_needs() {
    // A delete proves the server and the volume gone, and leaves the fences of both plus
    // the delete intent. A rental from those fences is refused by the provider runtime, so
    // a deploy after a finished delete must clear them.
    let root = root(&["fsn1"]);
    Record {
        phase: Phase::Deleted,
        image: Some(IMAGE.into()),
        server_type: Some("ccx13".into()),
        location: Some("fsn1".into()),
        volume_gb: Some(DEFAULT_VOLUME_GB),
        operation: CreateState::Terminated { worker_id: "42".into() },
        journal: Journal {
            location: Some("fsn1".into()),
            volume: CreateState::Terminated { worker_id: "9".into() },
            deleting: true,
            ..Journal::default()
        },
        ..Record::default()
    }
    .save(root.path())
    .unwrap();

    let begun = super::begin(root.path(), &Operation::Deploy(deploy())).unwrap();
    assert_eq!(begun.phase, Phase::Deploying);
    let recorded = Record::load(root.path()).unwrap();
    assert_eq!(
        recorded.operation,
        CreateState::Prepared,
        "the server fence names a deleted server"
    );
    assert_eq!(
        recorded.journal,
        Journal::default(),
        "the journal of the deleted machine, with its delete intent, is gone"
    );
    assert_eq!(recorded.image.as_deref(), Some(IMAGE), "the new request is recorded");
}

#[test]
fn an_explicit_delete_takes_over_a_run_that_a_crash_left_open() {
    // A status read answers the recorded phase, so a crash during a deploy leaves the
    // record at `deploying`. Only an explicit action may finish it, and a delete must be
    // possible then, or nothing can remove the machine.
    let root = root(&["fsn1"]);
    Record {
        phase: Phase::Deploying,
        image: Some(IMAGE.into()),
        operation: CreateState::Bound { worker_id: "42".into() },
        journal: Journal {
            location: Some("fsn1".into()),
            volume: CreateState::Bound { worker_id: "9".into() },
            ..Journal::default()
        },
        ..Record::default()
    }
    .save(root.path())
    .unwrap();
    assert_eq!(
        super::status(root.path()).unwrap().phase,
        Phase::Deploying,
        "a read reports the stale phase and changes nothing"
    );
    let begun = super::begin(root.path(), &Operation::Delete).unwrap();
    assert_eq!(begun.phase, Phase::Deleting);
    let begun = super::begin(root.path(), &Operation::Stop);
    assert!(begun.is_ok(), "a stop also takes the stale run over: {begun:?}");
}

#[test]
fn a_delete_and_a_stop_with_nothing_refuse() {
    let root = root(&["fsn1"]);
    Record::default().save(root.path()).unwrap();
    let nothing = super::begin(root.path(), &Operation::Delete).unwrap_err();
    assert!(nothing.to_string().contains("nothing to delete"), "{nothing}");
    let stop = super::begin(root.path(), &Operation::Stop).unwrap_err();
    assert!(stop.to_string().contains("nothing to stop"), "{stop}");
    // A run with nothing recorded is refused rather than renting something.
    assert!(super::run(root.path(), None, &Cancellation::default(), &mut |_| {}).is_err());
}

#[test]
fn the_ssh_target_names_the_person_who_signs_in() {
    let status = Status {
        phase: Phase::Ready,
        ssh_target: Some("root@203.0.113.10".into()),
        ssh_note: None,
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

#[test]
fn an_alias_of_the_managed_entry_is_its_own_ssh_host() {
    // The settings entry `browser_host` and the Device panel route both take this host, and
    // the block in the ssh configuration of the person turns it into the machine.
    let status = Status {
        phase: Phase::Ready,
        ssh_target: Some(ssh_entry::ALIAS.into()),
        ssh_note: None,
        image: None,
        server_type: None,
        location: None,
        volume_gb: None,
        hourly_net: None,
        error: None,
    };
    assert_eq!(status.ssh_host(), Some("horizon-browser-vm"));
}

/// A home of a person with one host of their own, and a cloud root beside it.
fn entry_home() -> (tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let ssh = home.path().join(".ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(ssh.join("config"), "Host laptop\n  HostName 192.0.2.44\n").unwrap();
    (home, tempfile::tempdir().unwrap())
}

/// A scan that offers no key at all, so a test never runs a scan of a machine.
fn silent_scan() -> ssh_entry::HostKeyScan {
    ssh_entry::HostKeyScan {
        program: PathBuf::from("true"),
        attempts: 1,
        connect_timeout_secs: 1,
        pause: std::time::Duration::ZERO,
    }
}

#[test]
fn a_horizon_that_does_not_know_the_home_writes_no_entry() {
    let root = tempfile::tempdir().unwrap();
    let (target, note) = ssh_entry::machine_route(None, root.path(), "root@203.0.113.10", Path::new(IDENTITY));

    assert_eq!(
        target, "root@203.0.113.10",
        "every route keeps the address of the machine"
    );
    let note = note.expect("the reason the entry is missing is recorded");
    assert!(note.contains("horizon-browser-vm"), "{note}");
    assert!(note.contains("home"), "{note}");
}

#[test]
fn the_name_of_a_person_is_reported_rather_than_taken_over() {
    // A configuration that already names the alias outside the block belongs to the person.
    let (home, root) = entry_home();
    let taken = format!(
        "Host laptop\n  HostName 192.0.2.44\nHost {}\n  HostName 192.0.2.45\n",
        ssh_entry::ALIAS
    );
    std::fs::write(home.path().join(".ssh").join("config"), &taken).unwrap();

    let (target, note) =
        ssh_entry::machine_route(Some(home.path()), root.path(), "root@203.0.113.10", Path::new(IDENTITY));

    assert_eq!(target, "root@203.0.113.10", "the deploy still names a route");
    let note = note.expect("the reason the entry is missing is recorded");
    assert!(note.contains(ssh_entry::ALIAS), "{note}");
    assert_eq!(
        std::fs::read_to_string(home.path().join(".ssh").join("config")).unwrap(),
        taken,
        "the configuration of the person is unchanged"
    );
}

#[test]
fn a_deploy_names_the_alias_and_a_stop_removes_the_entry() {
    let (home, root) = entry_home();
    let scan = silent_scan();

    // The deploy writes the block and records the alias.
    let (target, note) = ssh_entry::route(
        &ssh_entry::Entry::with_scan(home.path(), root.path(), scan.clone()),
        "root@203.0.113.10",
        Path::new(IDENTITY),
    );
    assert_eq!(target, ssh_entry::ALIAS);
    assert!(
        note.is_some(),
        "a scan that offers no key is reported, and the entry stays"
    );
    let written = std::fs::read_to_string(home.path().join(".ssh").join("config")).unwrap();
    assert!(written.contains("  HostName 203.0.113.10\n"), "{written}");

    let ready = Record {
        phase: Phase::Ready,
        ssh_target: Some(target.clone()),
        ssh_note: note,
        ..Record::default()
    };
    ready.save(root.path()).unwrap();

    // A stop records a machine that is gone, and takes the entry with it.
    let stopped = Saved::new(root.path(), ready).settled::<()>(Ok(()), Phase::Stopped, Some(home.path()));
    let stopped = stopped.unwrap();
    assert_eq!(stopped.ssh_target, None);
    let config = std::fs::read_to_string(home.path().join(".ssh").join("config")).unwrap();
    assert_eq!(
        config, "Host laptop\n  HostName 192.0.2.44\n",
        "the host of the person stays"
    );
    assert_eq!(super::status(root.path()).unwrap().ssh_target, None, "a read agrees");

    // A resume writes the entry again, so the route of the person survives a new address.
    let (resumed, _) = ssh_entry::route(
        &ssh_entry::Entry::with_scan(home.path(), root.path(), scan),
        "root@198.51.100.7",
        Path::new(IDENTITY),
    );
    assert_eq!(resumed, ssh_entry::ALIAS);
    let written = std::fs::read_to_string(home.path().join(".ssh").join("config")).unwrap();
    assert!(written.contains("  HostName 198.51.100.7\n"), "{written}");
}

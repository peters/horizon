use super::*;
use crate::browser::manifest::write_at;

const HOST_A: &str = "host-a";
const HOST_B: &str = "host-b";

fn member() -> AgentIdentity<'static> {
    AgentIdentity::new("horizon:agent-a", Some(HOST_A))
}

fn stamp(host: &str, actors: &[&str]) -> ManifestWorkspace {
    ManifestWorkspace::new(host, "ws-a", actors.iter().map(|actor| (*actor).to_string()).collect())
}

#[test]
fn only_horizon_agent_identities_are_workspace_scoped() {
    assert!(actor_is_workspace_scoped("horizon:agent-a"));
    assert!(!actor_is_workspace_scoped("horizon:"));
    assert!(!actor_is_workspace_scoped("horizon-mcp:4242"));
    assert!(!actor_is_workspace_scoped("browser-cli-test"));
    assert!(valid_host_instance(host_instance()));
    assert_eq!(host_instance(), host_instance(), "one identity per process");
    assert!(!valid_host_instance(""));
    assert!(!valid_host_instance("bad\ninstance"));

    let mut manifest = driver_manifest(Some(HOST_A));
    let external = AgentIdentity::new("browser-cli-test", None);
    assert!(manifest.permits(external), "unscoped identities need no stamp");
    assert!(!manifest.permits(member()), "unstamped manifests fail closed");
    assert_eq!(
        permit(&manifest, member()).expect_err("outside workspace").kind(),
        std::io::ErrorKind::PermissionDenied
    );
    manifest.workspace = Some(stamp(HOST_A, &["horizon:agent-a"]));
    assert!(manifest.permits(member()));
    manifest.host = Some(HOST_B.to_string());
    assert!(
        !manifest.permits(member()),
        "a stamp left by a previous host is stale once another host runs the driver"
    );
    manifest.host = None;
    assert!(
        !manifest.permits(member()),
        "a host-less manifest never matches a stamp"
    );
    manifest.host = Some(HOST_A.to_string());
    assert!(!manifest.permits(AgentIdentity::new("horizon:agent-b", Some(HOST_A))));
    assert!(
        !manifest.permits(AgentIdentity::new("horizon:agent-a", Some(HOST_B))),
        "the same persisted actor id under another live host is a different agent"
    );
    assert!(
        !manifest.permits(AgentIdentity::new("horizon:agent-a", None)),
        "a Horizon agent without a forwarded host instance never matches"
    );
    permit(&manifest, member()).expect("member is permitted");
}

#[test]
fn audit_reads_are_gated_by_workspace_membership_under_the_manifest_lock() {
    let root = tempfile::tempdir().expect("isolated root");
    let panel = "panel";
    write_at(
        &manifest_path_for_root(root.path(), panel),
        &BrowserManifest {
            workspace: Some(stamp(HOST_A, &["horizon:agent-a"])),
            ..driver_manifest(Some(HOST_A))
        },
    )
    .expect("write manifest");
    audit::append_at_path(
        &audit::audit_path_for_root(root.path(), panel),
        &BrowserAuditEntry::new(
            "action-1".to_string(),
            horizon_browser::BrowserAuditActor::Agent {
                name: "horizon:agent-a".to_string(),
            },
            horizon_browser::BrowserAuditStatus::Completed,
            horizon_browser::BrowserAuditAction::Reload,
        ),
    )
    .expect("append audit");

    let entries = read_audit_for_at(root.path(), panel, member()).expect("member reads audit");
    assert_eq!(entries.len(), 1);
    for outsider in [
        AgentIdentity::new("horizon:agent-b", Some(HOST_A)),
        AgentIdentity::new("horizon:agent-a", Some(HOST_B)),
        AgentIdentity::new("horizon:agent-a", None),
    ] {
        assert_eq!(
            read_audit_for_at(root.path(), panel, outsider)
                .expect_err("non-member is refused")
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }
    assert_eq!(
        read_audit_for_at(root.path(), panel, AgentIdentity::new("browser-cli-test", None))
            .expect("unscoped identity reads audit")
            .len(),
        1
    );
    assert_eq!(
        read_audit_for_at(root.path(), "missing", member())
            .expect_err("missing panel")
            .kind(),
        std::io::ErrorKind::NotFound
    );
}

#[test]
fn membership_is_exact_and_an_unstamped_manifest_authorizes_nobody() {
    let workspace = ManifestWorkspace::new(
        HOST_A,
        "ws-a",
        vec![
            "horizon:agent-b".to_string(),
            "horizon:agent-a".to_string(),
            "horizon:agent-a".to_string(),
        ],
    );
    assert_eq!(workspace.actors, ["horizon:agent-a", "horizon:agent-b"]);
    assert!(workspace.authorizes(member()));
    assert!(!workspace.authorizes(AgentIdentity::new("horizon:agent-c", Some(HOST_A))));
    assert!(!workspace.authorizes(AgentIdentity::new("horizon:agent", Some(HOST_A))));

    let mut manifest = driver_manifest(Some(HOST_A));
    assert!(!manifest.authorizes(member()));
    manifest.workspace = Some(workspace);
    assert!(manifest.authorizes(member()));
    assert!(!manifest.authorizes(AgentIdentity::new("horizon:agent-c", Some(HOST_A))));
}

fn driver_manifest(host: Option<&str>) -> BrowserManifest {
    BrowserManifest {
        panel_local_id: "panel".to_string(),
        host: host.map(str::to_string),
        ..BrowserManifest::default()
    }
}

#[test]
fn host_state_sync_writes_only_when_presentation_or_membership_changes() {
    let root = tempfile::tempdir().expect("isolated root");
    let path = manifest_path_for_root(root.path(), "panel");
    write_at(&path, &driver_manifest(Some(HOST_A))).expect("write manifest");
    let workspace = stamp(HOST_A, &["horizon:agent-a"]);

    assert!(sync_host_state_at(&path, "panel", true, &workspace).expect("stamp"));
    let stamped = read_at(&path).expect("read stamped");
    assert!(!stamped.hidden);
    assert_eq!(stamped.workspace.as_ref(), Some(&workspace));
    assert!(!sync_host_state_at(&path, "panel", true, &workspace).expect("steady state"));

    assert!(sync_host_state_at(&path, "panel", false, &workspace).expect("hide"));
    assert!(read_at(&path).expect("read hidden").hidden);

    // The member holds the lease and a pending handoff when the host moves
    // the panel to a workspace that no longer contains it.
    let now = now_millis();
    update_at(&path, "panel", |manifest| {
        manifest.owner = Some(crate::browser::manifest::ManifestOwner {
            name: "horizon:agent-a".to_string(),
            tty: None,
            updated_at: now,
        });
        manifest.handoff = Some(crate::browser::manifest::ManifestHandoff {
            request_id: "request-1".to_string(),
            reason: "sign in".to_string(),
            requested_at: now,
            done: false,
        });
    })
    .expect("seed lease and handoff");
    assert!(
        !sync_host_state_at(&path, "panel", false, &workspace).expect("same placement"),
        "an unchanged placement keeps the lease and handoff"
    );
    assert!(read_at(&path).expect("read").handoff_pending().is_some());

    let moved = ManifestWorkspace::new(HOST_A, "ws-b", vec!["horizon:agent-b".to_string()]);
    assert!(sync_host_state_at(&path, "panel", false, &moved).expect("move"));
    let after_move = read_at(&path).expect("read moved");
    assert!(after_move.authorizes(AgentIdentity::new("horizon:agent-b", Some(HOST_A))));
    assert!(!after_move.authorizes(member()));
    assert!(
        after_move.owner.is_none(),
        "a move revokes the lease of an owner the new stamp no longer permits"
    );
    assert!(
        after_move.handoff.is_none(),
        "a move clears that owner's pending handoff"
    );

    let other_host = stamp(HOST_B, &["horizon:agent-a"]);
    assert_eq!(
        sync_host_state_at(&path, "panel", false, &other_host)
            .expect_err("another live host must not rewrite the stamp")
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(
        read_at(&path)
            .expect("read unchanged")
            .authorizes(AgentIdentity::new("horizon:agent-b", Some(HOST_A))),
        "the driver host's stamp survives a foreign sync"
    );

    let legacy = manifest_path_for_root(root.path(), "legacy");
    write_at(&legacy, &driver_manifest(None)).expect("write legacy manifest");
    assert_eq!(
        sync_host_state_at(&legacy, "legacy", true, &workspace)
            .expect_err("a manifest without a recorded host is never stamped")
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );

    let missing = manifest_path_for_root(root.path(), "missing");
    assert_eq!(
        sync_host_state_at(&missing, "missing", true, &workspace)
            .expect_err("missing manifest must fail")
            .kind(),
        std::io::ErrorKind::NotFound
    );
}

#[test]
fn requested_panels_are_stamped_and_claimed_in_one_transaction() {
    let root = tempfile::tempdir().expect("isolated root");
    let path = manifest_path_for_root(root.path(), "panel");
    write_at(&path, &driver_manifest(Some(HOST_A))).expect("write manifest");
    let workspace = stamp(HOST_A, &["horizon:agent-a", "horizon:agent-b"]);

    assert_eq!(
        publish_requested_panel_at(
            &path,
            "panel",
            false,
            &workspace,
            AgentIdentity::new("horizon:agent-c", Some(HOST_A))
        )
        .expect_err("an owner outside the workspace is refused")
        .to_string(),
        OUTSIDE_WORKSPACE_MESSAGE
    );
    let untouched = read_at(&path).expect("read");
    assert!(untouched.owner.is_none());
    assert!(
        untouched.workspace.is_none(),
        "a refused request must not stamp the panel"
    );
    assert!(!untouched.hidden, "a refused request must not change visibility");

    publish_requested_panel_at(&path, "panel", false, &workspace, member()).expect("stamp and claim");
    let published = read_at(&path).expect("read published");
    assert!(published.hidden);
    assert_eq!(published.workspace.as_ref(), Some(&workspace));
    assert_eq!(
        published.owner.as_ref().map(|owner| owner.name.as_str()),
        Some("horizon:agent-a")
    );
    assert!(published.authorizes(member()));

    assert_eq!(
        publish_requested_panel_at(
            &path,
            "panel",
            true,
            &workspace,
            AgentIdentity::new("horizon:agent-b", Some(HOST_A))
        )
        .expect_err("a live owner blocks a second requester")
        .to_string(),
        "browser panel already has another live owner"
    );
    let still_published = read_at(&path).expect("read after refusal");
    assert!(
        still_published.hidden,
        "a refused second requester must not change visibility"
    );
    assert_eq!(still_published.workspace.as_ref(), Some(&workspace));
    assert_eq!(
        still_published.owner.as_ref().map(|owner| owner.name.as_str()),
        Some("horizon:agent-a")
    );
    assert_eq!(
        publish_requested_panel_at(
            &path,
            "panel",
            true,
            &stamp(HOST_B, &["horizon:agent-a"]),
            AgentIdentity::new("horizon:agent-a", Some(HOST_B))
        )
        .expect_err("another host cannot publish this panel")
        .to_string(),
        OTHER_HOST_MESSAGE
    );
    assert_eq!(
        publish_requested_panel_at(
            &manifest_path_for_root(root.path(), "missing"),
            "missing",
            true,
            &workspace,
            member()
        )
        .expect_err("missing manifest")
        .kind(),
        std::io::ErrorKind::NotFound
    );
}

#[test]
fn legacy_manifests_deserialize_without_a_workspace_stamp() {
    let manifest: BrowserManifest = serde_json::from_str(
        r#"{
            "panel_local_id": "legacy",
            "browser_ws": "",
            "target_id": "",
            "url": "",
            "title": "",
            "user_active": false,
            "user_active_at": 0,
            "updated_at": 0
        }"#,
    )
    .expect("legacy manifest parses");
    assert!(manifest.workspace.is_none());
    assert!(!manifest.authorizes(member()));

    let stamped_without_host: BrowserManifest = serde_json::from_str(
        r#"{
            "panel_local_id": "old-stamp",
            "browser_ws": "",
            "target_id": "",
            "url": "",
            "title": "",
            "workspace": { "local_id": "ws-a", "actors": ["horizon:agent-a"] },
            "user_active": false,
            "user_active_at": 0,
            "updated_at": 0
        }"#,
    )
    .expect("host-less stamp parses");
    assert!(
        !stamped_without_host.permits(member()),
        "a stamp without a host instance never matches a live host"
    );
}

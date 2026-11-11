use super::{KeptPanel, ManifestFailure, publish_kept_panel_failure, release_kept_panel_failure};
use crate::manifest::{
    AgentIdentity, BrowserManifest, HostStampOutcome, ManifestWorkspace, manifest_path_for_root, read_at, write_at,
};

const HOST_A: &str = "host-a";
const PANEL: &str = "panel-1";

fn workspace() -> ManifestWorkspace {
    ManifestWorkspace::new(HOST_A, "ws-a", vec!["horizon:agent-a".to_string()])
}

fn kept(failure: ManifestFailure) -> KeptPanel<'static> {
    KeptPanel {
        workspace: workspace(),
        hidden: false,
        url: "https://example.test/inbox",
        title: "Inbox",
        backend: horizon_browser::BackendKind::ChromiumCdp,
        failure,
    }
}

fn connection_lost() -> ManifestFailure {
    ManifestFailure::new(
        "attached_host_connection_lost",
        "the browser of browser-vm in the profile mail lost its connection while this panel had it open",
    )
}

#[test]
fn a_kept_panel_publishes_its_reason_for_the_agents_of_its_workspace() {
    let root = tempfile::tempdir().expect("root");
    let path = manifest_path_for_root(root.path(), PANEL);

    assert_eq!(
        publish_kept_panel_failure(root.path(), PANEL, &kept(connection_lost())).expect("publish"),
        HostStampOutcome::Written
    );

    let manifest = read_at(&path).expect("the reason the agents read");
    assert_eq!(manifest.panel_local_id, PANEL);
    assert_eq!(manifest.url, "https://example.test/inbox");
    assert_eq!(manifest.title, "Inbox");
    assert_eq!(manifest.backend, horizon_browser::BackendKind::ChromiumCdp);
    assert_eq!(manifest.failure.as_ref(), Some(&connection_lost()));
    assert_eq!(manifest.host.as_deref(), Some(HOST_A));
    assert!(
        manifest.authorizes(AgentIdentity::new("horizon:agent-a", Some(HOST_A))),
        "the agent that shares the panel's workspace reads the reason"
    );
    assert!(
        !manifest.authorizes(AgentIdentity::new("horizon:agent-b", Some(HOST_A))),
        "another workspace shows no kept panel"
    );
}

#[test]
fn the_same_reason_is_written_once_and_cleared_or_removed_once() {
    let root = tempfile::tempdir().expect("root");
    let path = manifest_path_for_root(root.path(), PANEL);
    publish_kept_panel_failure(root.path(), PANEL, &kept(connection_lost())).expect("publish");

    assert_eq!(
        publish_kept_panel_failure(root.path(), PANEL, &kept(connection_lost())).expect("republish"),
        HostStampOutcome::Unchanged,
        "an unchanged reason writes nothing"
    );

    assert_eq!(
        release_kept_panel_failure(root.path(), PANEL, false).expect("clear"),
        HostStampOutcome::Written
    );
    assert_eq!(
        read_at(&path).expect("manifest").failure,
        None,
        "a panel that connects again keeps its manifest without the reason"
    );
    assert_eq!(
        release_kept_panel_failure(root.path(), PANEL, true).expect("remove"),
        HostStampOutcome::NotOwned,
        "a manifest without a reason is never a kept panel's to remove"
    );

    publish_kept_panel_failure(root.path(), PANEL, &kept(connection_lost())).expect("publish again");
    assert_eq!(
        release_kept_panel_failure(root.path(), PANEL, true).expect("remove"),
        HostStampOutcome::Written
    );
    assert!(read_at(&path).is_none(), "a panel that left the board leaves the list");
}

#[test]
fn a_manifest_of_another_host_is_not_this_hosts_to_describe() {
    let root = tempfile::tempdir().expect("root");
    let path = manifest_path_for_root(root.path(), PANEL);
    write_at(
        &path,
        &BrowserManifest {
            panel_local_id: PANEL.to_string(),
            host: Some("another-horizon".to_string()),
            ..BrowserManifest::default()
        },
    )
    .expect("another host's manifest");

    assert_eq!(
        publish_kept_panel_failure(root.path(), PANEL, &kept(connection_lost())).expect("publish"),
        HostStampOutcome::NotOwned
    );
    assert_eq!(read_at(&path).expect("manifest").failure, None);
}

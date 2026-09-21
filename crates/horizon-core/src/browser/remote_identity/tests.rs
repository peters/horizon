use horizon_browser::{
    BackendKind, DeviceEvidenceSource, RemoteDeviceIdentity, RemoteReleaseOutcome, RemoteSessionEvent,
    RemoteSessionRequest, remote::DeviceRequirement,
};
use serde_json::json;

use super::RemoteIdentityDisplay;
use crate::browser::{BrowserConfig, BrowserPanelState};

fn requested() -> RemoteIdentityDisplay {
    RemoteIdentityDisplay::requested(&RemoteSessionRequest {
        adapter: horizon_browser::remote::RemoteAdapterKind::Webdriver,
        recovery: horizon_browser::RemoteAllocation::default(),
        endpoint: "https://example.test".into(),
        authorization: None,
        capabilities: json!({"browserName": "MicrosoftEdge", "platformName": "Windows"}),
        allocation_timeout: std::time::Duration::from_secs(1),
        max_session: std::time::Duration::from_secs(1),
        idle_release: std::time::Duration::from_secs(1),
        label: "private-target".into(),
        provider: "private-provider".into(),
        quota_key: "private-quota".into(),
        browser: BackendKind::ChromiumCdp,
        device: DeviceRequirement::default(),
        evidence: DeviceEvidenceSource::BrowserstackSession {
            api_endpoint: "https://example.test".into(),
        },
    })
}

#[test]
fn desktop_identity_uses_confirmed_browser_and_os_without_claiming_physical_hardware() {
    let mut display = requested();
    assert_eq!(display.label(), "BrowserStack · Awaiting confirmation");
    assert!(display.tooltip().contains("Requested: Edge · Windows"));
    display.confirm(&RemoteDeviceIdentity {
        browser_name: Some("edge".into()),
        browser_version: Some("152".into()),
        os_name: Some("Windows".into()),
        os_version: Some("10".into()),
        ..RemoteDeviceIdentity::default()
    });
    assert_eq!(display.label(), "BrowserStack · Edge · Windows 10");
    assert!(display.tooltip().contains("Browser version: 152"));
    assert!(display.tooltip().contains("unverified hardware"));
    assert!(!display.tooltip().contains("physical device"));
    assert!(!display.tooltip().contains("private-"));
}

#[test]
fn mobile_identity_uses_friendly_names_and_preserves_the_entire_device_name() {
    let mut display = requested();
    let model = "iPhone 16 Pro Max with a very long provider-reported model name";
    display.confirm(&RemoteDeviceIdentity {
        browser_name: Some("iphone".into()),
        os_name: Some("ios".into()),
        os_version: Some("18.5".into()),
        model: Some(model.into()),
        hardware: Some(horizon_browser::DeviceEvidence::Physical),
        ..RemoteDeviceIdentity::default()
    });
    assert_eq!(display.label(), format!("BrowserStack · Safari · {model} · iOS 18.5"));
    assert!(display.tooltip().contains(model));
    assert!(display.tooltip().contains("physical device"));
}

#[test]
fn missing_evidence_does_not_promote_the_requested_configuration() {
    let mut display = requested();
    display.confirm(&RemoteDeviceIdentity::default());
    assert_eq!(display.label(), "BrowserStack · Browser unconfirmed · OS unconfirmed");
    assert!(display.tooltip().contains("Requested: Edge · Windows"));
    assert!(display.tooltip().contains("Device model: not reported"));
}

#[test]
fn session_replacement_and_end_clear_previous_identity() {
    let mut state = BrowserPanelState::inert_remote("target", "provider");
    let confirm = |state: &mut BrowserPanelState| {
        state.apply_remote_session_event_for_tests(RemoteSessionEvent::DeviceIdentity {
            label: "target".into(),
            identity: RemoteDeviceIdentity {
                browser_name: Some("edge".into()),
                model: Some("old-model".into()),
                ..RemoteDeviceIdentity::default()
            },
        });
    };
    confirm(&mut state);
    state.apply_remote_session_event_for_tests(RemoteSessionEvent::DeviceIdentity {
        label: "target".into(),
        identity: RemoteDeviceIdentity::default(),
    });
    assert!(
        !state
            .remote_identity_display()
            .expect("remote")
            .tooltip()
            .contains("old-model")
    );
    confirm(&mut state);
    state.apply_remote_session_event_for_tests(RemoteSessionEvent::Released {
        label: "target".into(),
        outcome: RemoteReleaseOutcome::ReleaseUnknown {
            attempts: 1,
            reason: "unknown".into(),
        },
    });
    assert_eq!(state.remote_device(), None);
    assert!(
        !state
            .remote_identity_display()
            .expect("remote")
            .tooltip()
            .contains("old-model")
    );
    assert!(
        state.holds_remote_allocation(),
        "clearing the label does not release the provider quota"
    );
    assert!(
        state
            .remote_identity_display()
            .expect("remote")
            .label()
            .ends_with("Release unconfirmed")
    );
    assert!(
        state
            .remote_identity_display()
            .expect("remote")
            .tooltip()
            .contains("may still hold")
    );
    confirm(&mut state);
    state.stop();
    assert_eq!(state.remote_device(), None);
    assert!(
        state
            .remote_identity_display()
            .expect("remote")
            .label()
            .ends_with("Release unconfirmed")
    );
    state.apply_remote_session_event_for_tests(RemoteSessionEvent::Released {
        label: "target".into(),
        outcome: RemoteReleaseOutcome::Released,
    });
    assert!(
        state
            .remote_identity_display()
            .expect("remote")
            .label()
            .ends_with("Session ended")
    );
    assert!(!state.holds_remote_allocation());
    let restored = BrowserPanelState::restored_remote("restored", &BrowserConfig::default(), "target".into(), None);
    assert!(
        restored
            .remote_identity_display()
            .expect("remote")
            .label()
            .ends_with("Session ended")
    );
    assert!(BrowserPanelState::inert().remote_identity_display().is_none());
}

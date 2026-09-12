use super::{
    AzureAccessToken, AzureCliCredential, AzureDiskSku, AzureError, AzureLifecycle, AzureProfile,
    credential::parse_cli_token, valid_identity_id, valid_location, valid_registry_login_server, valid_vm_size,
};
use crate::cloud_run::{CloudProvider, WorkerLifetime, WorkerTarget};
mod deployment;
mod identity;
mod provider;
mod transport;
use std::time::Duration;

pub(super) const SUB: &str = "0f0e0d0c-0b0a-4908-8706-050403020100";
pub(super) const OTHER_SUB: &str = "9a8b7c6d-5e4f-4a3b-9c2d-1e0f9a8b7c6d";
pub(super) const GROUP: &str = "horizon-ws-sample";
const ED25519_BLOB_PREFIX: &[u8] = b"\0\0\0\x0bssh-ed25519\0\0\0\x20";

/// A structurally valid Ed25519 public key whose 32 key bytes are all `byte`.
pub(super) fn ed25519_key(byte: u8, comment: &str) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let blob = [ED25519_BLOB_PREFIX, &[byte; 32]].concat();
    let comment = if comment.is_empty() {
        String::new()
    } else {
        format!(" {comment}")
    };
    format!("ssh-ed25519 {}{comment}", STANDARD.encode(blob))
}
pub(super) const IMAGE: &str =
    "example.azurecr.io/horizon-remote-worker@sha256:20cc03ef2530336b7374cc35412c8583b1422726c630ec6e6cd1450d690a74f6";

pub(super) fn profile() -> AzureProfile {
    AzureProfile {
        name: "cpu-north".into(),
        subscription_id: SUB.into(),
        location: "northeurope".into(),
        vm_size: "Standard_D4s_v3".into(),
        image_pull_identity_id: format!(
            "/subscriptions/{SUB}/resourceGroups/horizon-worker-registry/providers/Microsoft.ManagedIdentity/userAssignedIdentities/puller"
        ),
        declared_hourly_cost_micros: 200_000,
        registry_login_server: "example.azurecr.io".into(),
        disk_sku: AzureDiskSku::PremiumLrs,
    }
}

pub(super) fn target() -> WorkerTarget {
    WorkerTarget {
        provider: CloudProvider::Azure,
        profile: "cpu-north".into(),
        image: IMAGE.into(),
        disk_gib: 32,
        lifetime: WorkerLifetime::Persistent,
        max_hourly_cost_micros: Some(500_000),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

#[test]
fn profile_and_target_validation_reject_before_any_provider_call() {
    assert_eq!(profile().validate(), Ok(()));
    assert_eq!(profile().validate_target(&target()), Ok(()));
    let profile_mutations: [fn(&mut AzureProfile); 10] = [
        |p| p.subscription_id = "Finter As".into(),
        |p| p.subscription_id = SUB.to_ascii_uppercase(),
        |p| p.location = "North Europe".into(),
        |p| p.vm_size = "D4s_v3".into(),
        |p| p.vm_size = "Standard_".into(),
        |p| p.image_pull_identity_id = p.image_pull_identity_id.replace(SUB, OTHER_SUB),
        |p| p.image_pull_identity_id = "/subscriptions/x".into(),
        |p| p.declared_hourly_cost_micros = 0,
        |p| p.registry_login_server = "example.docker.io".into(),
        |p| p.name = " cpu ".into(),
    ];
    for mutate in profile_mutations {
        let mut profile = profile();
        mutate(&mut profile);
        assert_eq!(profile.validate(), Err(AzureError::InvalidProfile));
        assert_eq!(profile.validate_target(&target()), Err(AzureError::InvalidProfile));
    }
    let target_mutations: [fn(&mut WorkerTarget); 7] = [
        |t| t.provider = CloudProvider::RunPod,
        |t| t.profile = "other".into(),
        |t| t.disk_gib = 0,
        |t| t.image = "example.azurecr.io/horizon-remote-worker:latest".into(),
        |t| t.image = IMAGE.replace("example.azurecr.io", "other.azurecr.io"),
        |t| t.image = IMAGE.replace("example.azurecr.io/", ""),
        |t| t.lifetime = WorkerLifetime::TimeLimited { seconds: 0 },
    ];
    for mutate in target_mutations {
        let mut target = target();
        mutate(&mut target);
        assert_eq!(profile().validate_target(&target), Err(AzureError::InvalidTarget));
    }
    let mut cheap = target();
    cheap.max_hourly_cost_micros = Some(100_000);
    assert_eq!(
        profile().validate_target(&cheap),
        Err(AzureError::DeclaredCostExceedsLimit {
            declared: 200_000,
            maximum: 100_000
        })
    );
    let mut exact = target();
    exact.max_hourly_cost_micros = Some(200_000);
    assert_eq!(profile().validate_target(&exact), Ok(()));
    let mut unlimited = target();
    unlimited.max_hourly_cost_micros = None;
    assert_eq!(profile().validate_target(&unlimited), Ok(()));
}

#[test]
fn validators_follow_azure_naming_rules() {
    let identity = profile().image_pull_identity_id;
    assert!(valid_identity_id(&identity, SUB));
    assert!(valid_identity_id(
        &identity.replace(SUB, &SUB.to_ascii_uppercase()),
        SUB
    ));
    assert!(valid_identity_id(
        &identity.replace("resourceGroups", "resourcegroups"),
        SUB
    ));
    assert!(valid_identity_id(&identity.replace("puller", &"n".repeat(128)), SUB));
    assert!(!valid_identity_id(&identity, OTHER_SUB));
    for (bad, label) in [
        (identity.replace("puller", "_puller"), "leading underscore"),
        (identity.replace("puller", "ab"), "too short"),
        (identity.replace("puller", &"n".repeat(129)), "too long"),
        (
            identity.replace("Microsoft.ManagedIdentity", "Microsoft.Compute"),
            "provider",
        ),
        (format!("{identity}/extra"), "trailing segments"),
        (
            identity.replace("/subscriptions/", "/Subscriptions/"),
            "fixed segment casing",
        ),
        (format!("{identity} "), "whitespace"),
    ] {
        assert!(!valid_identity_id(&bad, SUB), "{label}");
    }
    for location in ["northeurope", "eastus2", "swedencentral"] {
        assert!(valid_location(location), "{location}");
    }
    for location in ["", "x", "North Europe", "north-europe", "2northeurope", &"a".repeat(65)] {
        assert!(!valid_location(location), "{location}");
    }
    for server in [
        "example.azurecr.io",
        "horizonworkersa898ee.azurecr.io",
        "abc12.azurecr.io",
    ] {
        assert!(valid_registry_login_server(server), "{server}");
    }
    for server in [
        "Example.azurecr.io",
        "my-reg.azurecr.io",
        "abcd.azurecr.io",
        "example.azurecr.cn",
        "azurecr.io",
    ] {
        assert!(!valid_registry_login_server(server), "{server}");
    }
}

#[test]
fn vm_sizes_come_from_the_validated_allowlist() {
    use super::SUPPORTED_VM_SIZES;
    assert!(
        SUPPORTED_VM_SIZES.contains(&"Standard_D4s_v3"),
        "the live-validated candidate"
    );
    for size in SUPPORTED_VM_SIZES {
        assert!(valid_vm_size(size), "{size}");
        assert!(size.starts_with("Standard_") && !size.contains('p'), "{size}: x64 only");
    }
    for size in [
        "Standard_D4_v3",
        "Standard_A2_v2",
        "Standard_DS3_v2",
        "Standard_D4ps_v5",
        "Standard_B96s",
        "Standard_D0s_v3",
        "Standard_D999s_v3",
        "Standard_E4-8s_v5",
        "Standard_D4s_v2",
        "Standard_d4s_v3",
        "Standard_D4s_v3 ",
        "Standard_",
        "Basic_A1",
        "",
    ] {
        assert!(!valid_vm_size(size), "{size}");
    }
    let mut standard_disks = profile();
    standard_disks.disk_sku = AzureDiskSku::StandardSsdLrs;
    assert_eq!(standard_disks.validate(), Ok(()));
    let decoded: AzureProfile = serde_json::from_str(
        &serde_json::to_string(&profile())
            .expect("encode")
            .replace(",\"disk_sku\":\"Premium_LRS\"", ""),
    )
    .expect("decode");
    assert_eq!(decoded.disk_sku, AzureDiskSku::StandardSsdLrs, "the default SKU");
}

#[test]
fn lifecycle_mapping_keeps_billed_stop_apart_from_deallocated() {
    let map = AzureLifecycle::from_states;
    assert_eq!(
        map(Some("Succeeded"), Some("PowerState/running")),
        AzureLifecycle::Running
    );
    assert_eq!(
        map(Some("Succeeded"), Some("PowerState/deallocated")),
        AzureLifecycle::Deallocated
    );
    assert_eq!(
        map(Some("Succeeded"), Some("PowerState/stopped")),
        AzureLifecycle::StoppedAllocated
    );
    assert_eq!(
        map(Some("Succeeded"), Some("PowerState/starting")),
        AzureLifecycle::Transitioning
    );
    assert_eq!(
        map(Some("Succeeded"), Some("PowerState/deallocating")),
        AzureLifecycle::Transitioning
    );
    assert_eq!(map(Some("Succeeded"), None), AzureLifecycle::Unknown);
    assert_eq!(map(Some("Creating"), None), AzureLifecycle::Transitioning);
    assert_eq!(map(Some("Failed"), Some("PowerState/running")), AzureLifecycle::Failed);
    assert_eq!(map(Some("Canceled"), None), AzureLifecycle::Failed);
    assert_eq!(
        map(Some("Deleting"), Some("PowerState/running")),
        AzureLifecycle::Deleting
    );
    assert_eq!(map(None, Some("PowerState/running")), AzureLifecycle::Unknown);
    assert_eq!(
        map(Some("Succeeded"), Some("PowerState/unknown")),
        AzureLifecycle::Unknown
    );
}

#[test]
fn tokens_are_redacted_bounded_and_refreshed_near_expiry() {
    let token = AzureAccessToken::new("synthetic-token-value", Duration::from_secs(3_600)).expect("token");
    assert!(!token.needs_refresh());
    assert_eq!(format!("{token:?}"), "AzureAccessToken(<redacted>)");
    assert_eq!(token.authorization_header(), "Bearer synthetic-token-value");
    assert!(
        AzureAccessToken::new("synthetic", Duration::from_secs(60))
            .expect("token")
            .needs_refresh()
    );
    for bad in ["", " spaced token", "tab\tin", &"x".repeat(16 * 1024 + 1)] {
        assert!(AzureAccessToken::new(bad, Duration::from_secs(60)).is_err());
    }
    assert_eq!(
        AzureAccessToken::new("synthetic", Duration::MAX).expect_err("no overflow panic"),
        AzureError::CredentialUnavailable {
            reason: "token expiry is out of range"
        }
    );
    let later = unix_now() + 3_600;
    let numeric = format!(r#"{{"accessToken":"synthetic-token-value","expires_on":{later}}}"#);
    assert!(!parse_cli_token(numeric.as_bytes()).expect("parse").needs_refresh());
    let stringy = format!(r#"{{"accessToken":"synthetic-token-value","expires_on":"{later}"}}"#);
    assert!(!parse_cli_token(stringy.as_bytes()).expect("parse").needs_refresh());
    let expired = format!(
        r#"{{"accessToken":"synthetic-token-value","expires_on":{}}}"#,
        unix_now() + 60
    );
    let far_future = r#"{"accessToken":"synthetic-token-value","expires_on":18446744073709551615}"#;
    for (payload, reason) in [
        (&b"not json"[..], "Azure CLI token response was malformed"),
        (br#"{"expires_on":1}"#, "Azure CLI token response was malformed"),
        (
            br#"{"accessToken":"synthetic-token-value"}"#,
            "Azure CLI token response lacked an expiry",
        ),
        (expired.as_bytes(), "Azure CLI returned an expired token"),
        (far_future.as_bytes(), "token expiry is out of range"),
        (
            br#"{"accessToken":"","expires_on":9999999999}"#,
            "token has an invalid shape",
        ),
    ] {
        let error = parse_cli_token(payload).expect_err("rejected");
        assert_eq!(error, AzureError::CredentialUnavailable { reason });
        assert!(!format!("{error:?} {error}").contains("synthetic"));
    }
    assert_eq!(
        AzureCliCredential::new("not-a-uuid").err(),
        Some(AzureError::InvalidProfile)
    );
    let custom = std::path::PathBuf::from("/home/operator-name/bin/az");
    let credential = AzureCliCredential::with_executable(custom, SUB).expect("credential");
    let shown = format!("{credential:?}");
    assert!(!shown.contains(SUB), "subscription id stays out of Debug output");
    assert!(
        !shown.contains("operator-name"),
        "configured path stays out of Debug output"
    );
    assert!(shown.contains("\"az\""), "{shown}");
}

/// True once the process whose id is recorded in `pid_file` no longer accepts signals.
#[cfg(unix)]
fn gone_after_bound(pid_file: &std::path::Path) -> bool {
    let pid = std::fs::read_to_string(pid_file).expect("pid file").trim().to_string();
    assert!(pid.parse::<u32>().is_ok(), "{pid}");
    (0..60).any(|_| {
        std::thread::sleep(Duration::from_millis(50));
        let alive = std::process::Command::new("/bin/kill")
            .args(["-0", "--", &pid])
            .stderr(std::process::Stdio::null())
            .status();
        !alive.is_ok_and(|status| status.success())
    })
}

#[cfg(unix)]
#[test]
fn cli_credential_invokes_the_executable_without_a_shell_and_enforces_its_bounds() {
    use super::AzureCredentialSource as _;
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().expect("temp dir");
    let calls = directory.path().join("calls");
    let write_script = |name: &str, body: String| {
        let path = directory.path().join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n{body}", calls.display()),
        )
        .expect("script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        path
    };
    let token_json = format!(
        r#"{{"accessToken":"synthetic-token-value","expires_on":{}}}"#,
        unix_now() + 3_600
    );
    let good = write_script("fake-az", format!("printf '%s' '{token_json}'\n"));
    let failing = write_script("fake-az-fail", format!("printf '%s' '{token_json}'\nexit 1\n"));
    let sleep_pid = directory.path().join("sleep-pid");
    let slow = write_script(
        "fake-az-slow",
        format!("sleep 30 &\necho $! > '{}'\nwait $!\n", sleep_pid.display()),
    );
    let verbose = write_script("fake-az-verbose", "head -c 70000 /dev/zero | tr '\\0' a\n".into());
    let orphan_pid = directory.path().join("orphan-pid");
    let orphaning = write_script(
        "fake-az-orphaning",
        format!("sleep 30 &\necho $! > '{}'\nexit 0\n", orphan_pid.display()),
    );
    let credential = AzureCliCredential::with_executable(good, SUB).expect("credential");
    // Another test thread may fork while the script file is still open for writing, which
    // makes the first exec fail with "text file busy"; that window closes within milliseconds.
    let first = (0..20)
        .find_map(|_| {
            credential.token().ok().or_else(|| {
                std::thread::sleep(Duration::from_millis(25));
                None
            })
        })
        .expect("token");
    let second = credential.token().expect("cached token");
    assert_eq!(first.authorization_header(), second.authorization_header());
    assert_eq!(first.authorization_header(), "Bearer synthetic-token-value");
    let recorded = std::fs::read_to_string(&calls).expect("calls");
    assert_eq!(
        recorded.lines().count(),
        1,
        "second call served from the in-memory cache"
    );
    assert_eq!(
        recorded.trim(),
        format!(
            "account get-access-token --resource https://management.azure.com/ --subscription {SUB} --output json --only-show-errors"
        )
    );
    let unavailable = |reason| AzureError::CredentialUnavailable { reason };
    let token = |path, timeout, limit| {
        AzureCliCredential::with_executable(path, SUB)
            .expect("credential")
            .with_bounds(timeout, limit)
            .token()
            .expect_err("bounded failure")
    };
    assert_eq!(
        token(failing, Duration::from_secs(30), 64 * 1024),
        unavailable("Azure CLI did not return a token")
    );
    let started = std::time::Instant::now();
    assert_eq!(
        token(slow, Duration::from_millis(300), 64 * 1024),
        unavailable("Azure CLI timed out")
    );
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "the slow CLI is killed at the bound, not awaited"
    );
    let gone = gone_after_bound(&sleep_pid);
    assert!(gone, "the launcher's own child is ended with the process group");
    let started = std::time::Instant::now();
    assert_eq!(
        token(orphaning, Duration::from_millis(300), 64 * 1024),
        unavailable("Azure CLI timed out")
    );
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "an exited leader does not extend the bound"
    );
    assert!(
        gone_after_bound(&orphan_pid),
        "a descendant holding stdout is ended after the leader exited"
    );
    assert_eq!(
        token(verbose, Duration::from_secs(30), 64 * 1024),
        unavailable("Azure CLI did not return a token")
    );
    assert_eq!(
        token(directory.path().join("absent"), Duration::from_secs(30), 64 * 1024),
        unavailable("Azure CLI could not be started")
    );
}

#[cfg(unix)]
#[test]
fn cli_credential_bounds_a_budgeted_token_by_the_caller_not_by_its_own_timeout() {
    use super::AzureCredentialSource as _;
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().expect("temp dir");
    // One slow fake CLI per phase, each with its own start marker, so a phase can
    // only ever be satisfied by its own child.
    let slow = |name: &str| {
        let script = directory.path().join(name);
        let marker = directory.path().join(format!("{name}.started"));
        std::fs::write(&script, format!("#!/bin/sh\n: > '{}'\nsleep 30\n", marker.display())).expect("script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        let credential = AzureCliCredential::with_executable(script, SUB)
            .expect("credential")
            .with_bounds(Duration::from_secs(5), 64 * 1024);
        (std::sync::Arc::new(credential), marker)
    };
    let unavailable = |reason| AzureError::CredentialUnavailable { reason };
    // The caller's budget ends the CLI run long before the credential's own timeout,
    // and the timeout teardown is detached, so the call returns at the budget.
    let (credential, _) = slow("fake-az-slow-budget");
    let budget = Duration::from_millis(300);
    let started = std::time::Instant::now();
    assert_eq!(
        credential.token_within(budget).expect_err("budget"),
        unavailable("Azure CLI timed out")
    );
    assert!(
        started.elapsed() < budget + Duration::from_millis(250),
        "the call returns at the budget without waiting for the tree: {:?}",
        started.elapsed()
    );
    // A refresh already running in another thread holds the cache lock; a budgeted
    // caller waits at most its budget for it instead of the refresh's whole run.
    let (credential, marker) = slow("fake-az-slow-contended");
    // The fake CLI touches the marker once it runs, which happens under the lock.
    // Another test thread may fork while the script is still open for writing, which
    // makes an exec fail with "text file busy"; such an attempt ends at once without
    // a marker and is simply retried.
    let refresh = (0..5)
        .find_map(|_| {
            let refreshing = std::sync::Arc::clone(&credential);
            let refresh = std::thread::spawn(move || refreshing.token_within(Duration::from_secs(4)));
            let until = std::time::Instant::now() + Duration::from_secs(3);
            while !marker.exists() {
                if refresh.is_finished() {
                    assert_eq!(
                        refresh.join().expect("refresh thread").expect_err("early exit"),
                        unavailable("Azure CLI could not be started")
                    );
                    return None;
                }
                assert!(std::time::Instant::now() < until, "the refresh never started");
                std::thread::sleep(Duration::from_millis(10));
            }
            Some(refresh)
        })
        .expect("a refresh that started");
    let started = std::time::Instant::now();
    assert_eq!(
        credential
            .token_within(Duration::from_millis(200))
            .expect_err("lock wait"),
        unavailable("Azure CLI refresh in progress exceeded the caller's budget")
    );
    assert!(started.elapsed() < Duration::from_millis(450));
    assert_eq!(
        refresh.join().expect("refresh thread").expect_err("refresh"),
        unavailable("Azure CLI timed out")
    );
}

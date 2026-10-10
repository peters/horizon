//! Thin deployment adapter for cloud-only tailnet enrollment.
mod keychain;
use super::{Error, Result, command::Runner, ssh::Connection, state};
pub use horizon_cloud::tailnet::{Catalog, Selection, Store, Tailnet, valid_key};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};

#[must_use]
pub fn store(root: &Path) -> Store {
    Store::new(root.to_path_buf())
}
fn mapped(_: horizon_cloud::tailnet::Error) -> Error {
    Error::Invalid("Tailnet settings or OS credential are unavailable; open Settings > Tailnets")
}
#[derive(serde::Serialize)]
struct Enrollment<'a> {
    tailnet: Option<&'a str>,
    key: Option<&'a str>,
}
/// Called under the deployment lock, before any agent sessions are started.
/// # Errors
/// Refuses unavailable workers, unsafe permissions and inaccessible credentials.
pub fn configure(connection: &Connection, cloud: &Path, runner: &Runner<'_>) -> Result<()> {
    if !cloud.join("tailnet.json").exists() {
        return Ok(());
    }
    let selected = Selection::load(cloud).map_err(mapped)?;
    if selected.tailnet.is_none() {
        return Ok(());
    }
    let payload = |key: Option<&str>| {
        serde_json::to_string(&Enrollment {
            tailnet: selected.tailnet.as_deref(),
            key,
        })
        .map(zeroize::Zeroizing::new)
        .map_err(|_| Error::Json)
    };
    let invoke = |input: &str| {
        runner.private_exchange(
            &mut connection.pinned_command("horizon-worker-tailnet configure"),
            input.as_bytes(),
            Duration::from_secs(60),
        )
    };
    let reply = invoke(&payload(None)?)?;
    match reply.as_slice() {
        b"ready\n" => Ok(()),
        b"needs_key\n" => {
            let id = selected
                .tailnet
                .as_deref()
                .ok_or(Error::Invalid("Missing tailnet selection"))?;
            let root = cloud
                .parent()
                .ok_or(Error::Invalid("Missing cloud settings directory"))?;
            let slot = store(root).credential_slot(id).map_err(mapped)?;
            let key = keychain::read(&slot).map_err(mapped)?;
            let key = std::str::from_utf8(&key)
                .ok()
                .filter(|key| valid_key(key))
                .ok_or_else(|| mapped(horizon_cloud::tailnet::Error::Invalid))?;
            if invoke(&payload(Some(key))?)? == b"ready\n" {
                Ok(())
            } else {
                Err(Error::Invalid("Tailnet enrollment was not confirmed"))
            }
        }
        _ => Err(Error::Invalid("Unexpected tailnet enrollment response")),
    }
}
/// # Errors
/// Caller-visible settings errors; every write uses the existing cloud lifecycle lock.
pub fn select(root: &Path, cloud_id: &str, id: Option<&str>) -> Result<()> {
    change(root, cloud_id, id)
}

/// Choose a network before provisioning; a provisioned cloud retains its choice.
/// # Errors
/// Rejects changing the network after an allocation was requested, including uncertain
/// requests. Choose another network on a new cloud instead.
pub fn change(root: &Path, cloud_id: &str, id: Option<&str>) -> Result<()> {
    let cloud = state::cloud_directory(root, cloud_id)?;
    let lock = state::Store::lock(&cloud)?;
    let prior = Selection::load(&cloud).map_err(mapped)?;
    if prior.tailnet.as_deref() != id && super::companions::lifecycle::requested_tailnet(&cloud)?.is_some() {
        return Err(Error::Invalid(
            "Tailnet is reserved by a pending cloud operation; cancel it before changing networks",
        ));
    }
    if lock
        .load()?
        .is_some_and(|s| s.spec.is_some() || s.worker.is_some() || s.operation != super::CreateState::Prepared)
    {
        if prior.tailnet.as_deref() == id {
            return Ok(());
        }
        return Err(Error::Invalid(
            "Tailnet is chosen only when provisioning a cloud; create a new cloud to choose another network",
        ));
    }
    let catalog = store(root).load().map_err(mapped)?;
    Selection::save(&cloud, id, &catalog).map_err(mapped)
}

/// Refuse selection drift before any deployment or companion provider work.
pub(in crate::cloud_runtime) fn validate_pending(cloud: &Path) -> Result<()> {
    if let Some(requested) = super::companions::lifecycle::requested_tailnet(cloud)?
        && Selection::load(cloud).map_err(mapped)? != requested
    {
        return Err(Error::Invalid(
            "Tailnet differs from the pending cloud request; cancel and submit a new operation",
        ));
    }
    Ok(())
}

/// The device name last read from the worker, or the image's stable-name fallback.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct DeviceName {
    pub name: String,
    pub observed: bool,
}

impl DeviceName {
    /// A published DNS name includes its `MagicDNS` domain. A bare name does not.
    #[must_use]
    pub fn full_name(&self) -> bool {
        self.name.contains('.')
    }

    fn published(bytes: &[u8]) -> Option<Self> {
        #[derive(serde::Deserialize)]
        struct Snapshot {
            devices: Vec<Device>,
        }
        #[derive(serde::Deserialize)]
        struct Device {
            name: String,
        }
        let snapshot: Snapshot = serde_json::from_slice(bytes).ok()?;
        let name = snapshot.devices.into_iter().next()?.name;
        let name = name.strip_suffix('.').unwrap_or(&name);
        valid_dns_name(name).then(|| Self {
            name: name.into(),
            observed: true,
        })
    }

    fn derived(cloud_id: &str, stable: bool) -> Option<Self> {
        if !stable || !horizon_cloud::valid_id(cloud_id) {
            return None;
        }
        let label = cloud_id.to_ascii_lowercase().replace('_', "-");
        let mut name = format!("horizon-cloud-{label}");
        let digest_suffix = name.rsplit_once('-').is_some_and(|(_, suffix)| {
            suffix.len() == 20 && suffix.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
        // Match the worker helper's collision-free transformation of lossy/long IDs.
        if label != cloud_id || name.len() > 63 || name.ends_with('-') || digest_suffix {
            name.truncate(name.len().min(42));
            let prefix = name.trim_end_matches('-');
            let digest = Sha256::digest(cloud_id.as_bytes());
            let suffix = digest[..10]
                .iter()
                .flat_map(|byte| {
                    let hex = b"0123456789abcdef";
                    [
                        char::from(hex[usize::from(byte >> 4)]),
                        char::from(hex[usize::from(byte & 15)]),
                    ]
                })
                .collect::<String>();
            name = format!("{prefix}-{suffix}");
        }
        Some(Self { name, observed: false })
    }
}

fn valid_dns_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                && label.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

// The worker publishes atomic snapshots in one sequential five-second loop. Its first
// post-configure publication can finish a status read started before configure; the
// second publication observes a status read that started afterward.
const FRESH_SNAPSHOT: &str = r#"import os, sys, time
path = sys.argv[1]
deadline = time.monotonic() + float(sys.argv[2])
def generation(file):
    info = os.fstat(file.fileno())
    return info.st_dev, info.st_ino, info.st_mtime_ns, info.st_ctime_ns
try:
    with open(path, "rb") as file:
        previous = generation(file)
except OSError:
    previous = None
remaining = 2
while time.monotonic() < deadline:
    time.sleep(0.05)
    try:
        with open(path, "rb") as file:
            current = generation(file)
            if current == previous:
                continue
            previous = current
            remaining -= 1
            if remaining == 0:
                data = file.read(65537)
                if len(data) > 65536:
                    sys.exit(1)
                sys.stdout.buffer.write(data)
                sys.exit(0)
    except OSError:
        pass
sys.exit(1)
"#;

/// Read only the public device snapshot, without copying its peers into logs or state.
/// The read follows enrollment; every deployment/reconnection refreshes the observation.
/// # Errors
/// Propagates selection corruption or cancellation; an unavailable snapshot is optional.
pub(super) fn configure_and_record(
    connection: &Connection,
    store: &state::Store,
    state: &mut state::Deployment,
    contract: &super::worker_contract::WorkerContract,
    runner: &Runner<'_>,
) -> Result<()> {
    configure(connection, store.root(), runner)?;
    state.tailnet_device = device_name(connection, store.root(), &state.cloud_id, contract, runner)?;
    store.save(state)
}

fn device_name(
    connection: &Connection,
    cloud: &Path,
    cloud_id: &str,
    contract: &super::WorkerContract,
    runner: &Runner<'_>,
) -> Result<Option<DeviceName>> {
    let selected = Selection::load(cloud).map_err(mapped)?;
    observe_device_name(
        selected.tailnet.is_some(),
        cloud_id,
        contract.tailnet_stable_name,
        runner.cancel,
        || {
            runner.private_exchange(
                &mut connection.pinned_command("/usr/bin/python3 - /run/horizon-tailnet-devices/devices.json 12"),
                FRESH_SNAPSHOT.as_bytes(),
                Duration::from_secs(15),
            )
        },
    )
}

fn observe_device_name(
    selected: bool,
    cloud_id: &str,
    stable: bool,
    cancel: &horizon_cloud::Cancellation,
    read: impl FnOnce() -> Result<Vec<u8>>,
) -> Result<Option<DeviceName>> {
    if !selected {
        return Ok(None);
    }
    let report = read();
    cancel.check()?;
    Ok(report
        .ok()
        .as_deref()
        .and_then(DeviceName::published)
        .or_else(|| DeviceName::derived(cloud_id, stable)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_actual_names_and_unavailable_legacy_snapshots() {
        let cancel = horizon_cloud::Cancellation::default();
        assert!(
            observe_device_name(false, "cloud1", true, &cancel, || panic!(
                "no network read without a tailnet"
            ))
            .unwrap()
            .is_none()
        );
        let actual = observe_device_name(true, "cloud1", false, &cancel, || {
            Ok(br#"{"devices":[{"name":"renamed-1.example.ts.net."}]}"#.to_vec())
        })
        .unwrap()
        .unwrap();
        assert_eq!(actual.name, "renamed-1.example.ts.net");
        assert!(actual.observed);
        for stable in [false, true] {
            let missing =
                observe_device_name(true, "cloud1", stable, &cancel, || Err(Error::PrivateTransport)).unwrap();
            assert_eq!(missing.is_some(), stable);
        }
        let canceled = observe_device_name(true, "cloud1", true, &cancel, || {
            cancel.cancel();
            Err(Error::PrivateTransport)
        });
        assert!(canceled.is_err(), "cancellation never supplies a success fallback");
    }

    #[test]
    #[cfg(unix)] // Synthetic shell output exercises the Unix process transport.
    fn the_snapshot_read_is_bounded_without_log_events() {
        let cancel = horizon_cloud::Cancellation::default();
        let events = std::sync::Mutex::new(Vec::new());
        let emit = |event| events.lock().unwrap().push(event);
        let runner = Runner {
            cancel: &cancel,
            emit: &emit,
            secrets: vec![],
        };
        let identity = observe_device_name(true, "cloud1", true, &cancel, || {
            runner.private_exchange(
                std::process::Command::new("sh").args(["-c", "head -c 65537 /dev/zero"]),
                &[],
                Duration::from_secs(5),
            )
        })
        .unwrap()
        .unwrap();
        assert!(!identity.observed);
        assert_eq!(identity.name, "horizon-cloud-cloud1");
        assert!(
            events.lock().unwrap().is_empty(),
            "peer contents stay out of deployment logs"
        );
    }

    #[test]
    #[cfg(unix)] // The worker's Python reader runs against an isolated Unix snapshot fixture.
    fn a_fresh_snapshot_requires_two_publications_and_cancellation_never_falls_back() {
        const OBSERVE_READS: &str = r#"import os, sys
original = os.fstat
def observe(fd):
    result = original(fd)
    with open(sys.argv[3], "a") as output:
        output.write(str(result.st_ino) + "\n")
    return result
os.fstat = observe
exec(sys.stdin.read())
"#;
        let temp = tempfile::tempdir().unwrap();
        let snapshot = temp.path().join("devices.json");
        let reads = temp.path().join("reads");
        let cancel = horizon_cloud::Cancellation::default();
        for cancelled in [false, true] {
            let initial = publish_fixture(&snapshot, "old.example.ts.net");
            std::fs::write(&reads, "").unwrap();
            std::thread::scope(|scope| {
                let (send, receive) = std::sync::mpsc::channel();
                let fixture = &snapshot;
                let observations = &reads;
                let cancellation = &cancel;
                scope.spawn(move || {
                    let runner = Runner {
                        cancel: cancellation,
                        emit: &|_| panic!("snapshot reads never emit peer data"),
                        secrets: vec![],
                    };
                    let result = observe_device_name(true, "cloud1", true, cancellation, || {
                        runner.private_exchange(
                            std::process::Command::new("python3")
                                .args(["-c", OBSERVE_READS])
                                .arg(fixture)
                                .arg("12")
                                .arg(observations),
                            FRESH_SNAPSHOT.as_bytes(),
                            Duration::from_secs(15),
                        )
                    });
                    send.send(result).unwrap();
                });
                await_fixture_read(&reads, initial);
                // An in-flight status query may still publish its pre-configure name.
                let older = publish_fixture(&snapshot, "in-flight.example.ts.net");
                await_fixture_read(&reads, older);
                assert!(
                    receive.try_recv().is_err(),
                    "one publication cannot establish freshness"
                );
                if cancelled {
                    cancel.cancel();
                    assert!(receive.recv_timeout(Duration::from_secs(5)).unwrap().is_err());
                } else {
                    publish_fixture(&snapshot, "renamed-worker-7.example.ts.net.");
                    let actual = receive.recv_timeout(Duration::from_secs(5)).unwrap().unwrap().unwrap();
                    assert_eq!(actual.name, "renamed-worker-7.example.ts.net");
                    assert!(actual.observed);
                }
            });
        }
    }

    #[test]
    #[cfg(unix)] // A stopped Unix publisher cannot make a cached name a fresh observation.
    fn an_unconfirmed_snapshot_returns_only_the_contract_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let snapshot = temp.path().join("devices.json");
        publish_fixture(&snapshot, "cached.example.ts.net");
        let cancel = horizon_cloud::Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| panic!("snapshot reads never emit peer data"),
            secrets: vec![],
        };
        for stable in [false, true] {
            let identity = observe_device_name(true, "cloud1", stable, &cancel, || {
                runner.private_exchange(
                    std::process::Command::new("python3").arg("-").arg(&snapshot).arg("12"),
                    FRESH_SNAPSHOT.as_bytes(),
                    Duration::from_secs(1),
                )
            })
            .unwrap();
            assert_eq!(identity.is_some(), stable);
            if let Some(identity) = identity {
                assert_eq!(identity.name, "horizon-cloud-cloud1");
                assert!(!identity.observed);
            }
        }
    }

    #[cfg(unix)]
    fn publish_fixture(path: &Path, name: &str) -> u64 {
        use std::os::unix::fs::MetadataExt;
        let pending = path.with_extension("pending");
        std::fs::write(&pending, serde_json::json!({"devices": [{"name": name}]}).to_string()).unwrap();
        std::fs::rename(pending, path).unwrap();
        std::fs::metadata(path).unwrap().ino()
    }

    #[cfg(unix)]
    fn await_fixture_read(path: &Path, inode: u64) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if std::fs::read_to_string(path)
                .unwrap_or_default()
                .lines()
                .any(|line| line == inode.to_string())
            {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "reader did not observe the fixture generation"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn snapshot_keeps_the_first_devices_actual_dns_name() {
        let snapshot = br#"{"devices":[{"name":"renamed-worker-1.example.ts.net.","online":false},{"name":"peer.example.ts.net."}]}"#;
        let identity = DeviceName::published(snapshot).unwrap();
        assert_eq!(identity.name, "renamed-worker-1.example.ts.net");
        assert!(identity.observed);
        assert!(identity.full_name());
        let bare = DeviceName::published(br#"{"devices":[{"name":"old-random-container"}]}"#).unwrap();
        assert!(!bare.full_name());
    }

    #[test]
    fn an_invalid_self_never_uses_a_peers_name() {
        for json in [
            br#"{"devices":[]}"#.as_slice(),
            br#"{"devices":[{"name":""},{"name":"peer.example.ts.net"}]}"#,
            br#"{"devices":[{"name":"bad/name"}]}"#,
            br#"{"devices":[{"name":"bad\nname"}]}"#,
            br#"{"devices":[{"name":"-name.example"}]}"#,
            br#"{"devices":[{"name":"name..example"}]}"#,
            br#"{"devices":[{"name":"name.example.."}]}"#,
            br#"{"devices":[{"name":"name.example..."}]}"#,
            b"invalid",
        ] {
            assert!(DeviceName::published(json).is_none());
        }
    }

    #[test]
    fn only_v2_derives_a_name_and_matches_the_worker_helper() {
        assert!(DeviceName::derived("cloud-123", false).is_none());
        assert!(DeviceName::derived("invalid/cloud", true).is_none());
        let direct = DeviceName::derived("cloud-123", true).unwrap();
        assert_eq!(direct.name, "horizon-cloud-cloud-123");
        assert!(!direct.observed);
        assert!(!direct.full_name());
        for id in ["Mixed_ID", "name-", "a-0123456789abcdef0123", &"a".repeat(100)] {
            let name = DeviceName::derived(id, true).unwrap().name;
            assert!(valid_dns_name(&name));
            assert!(name.len() <= 63);
        }
        for (id, expected) in [
            ("Mixed_ID", "horizon-cloud-mixed-id-12af5122d9932e2b2577"),
            ("name-", "horizon-cloud-name-f0a4e7e61161383a47dc"),
            (
                "a-0123456789abcdef0123",
                "horizon-cloud-a-0123456789abcdef0123-ab1695dda518dd53fce3",
            ),
            (
                &"a".repeat(100),
                "horizon-cloud-aaaaaaaaaaaaaaaaaaaaaaaaaaaa-2816597888e4a0d3a36b",
            ),
        ] {
            assert_eq!(DeviceName::derived(id, true).unwrap().name, expected);
        }
        assert_ne!(
            DeviceName::derived("Mixed_ID", true),
            DeviceName::derived("mixed-id", true)
        );
    }
}

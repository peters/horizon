//! Real Secret Service regression, exclusively on the script's disposable bus.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::*;

struct Daemon(Child);

impl Daemon {
    fn start(root: &Path) -> Self {
        let log = std::fs::File::create(root.join("daemon.log")).unwrap();
        let mut child = Command::new("gnome-keyring-daemon")
            .args(["--foreground", "--unlock", "--components=secrets"])
            .arg(format!("--control-directory={}", root.join("control").display()))
            .stdin(Stdio::piped())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"synthetic-fixture-password")
            .unwrap();
        let mut daemon = Self(child);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(daemon.0.try_wait().unwrap().is_none(), "private keyring exited");
            let output = Command::new("gdbus")
                .args([
                    "call",
                    "--session",
                    "--dest",
                    "org.freedesktop.DBus",
                    "--object-path",
                    "/org/freedesktop/DBus",
                    "--method",
                    "org.freedesktop.DBus.GetConnectionUnixProcessID",
                    "org.freedesktop.secrets",
                ])
                .output()
                .unwrap();
            if output.status.success()
                && String::from_utf8_lossy(&output.stdout).trim() == format!("(uint32 {},)", daemon.0.id())
            {
                return daemon;
            }
            assert!(
                Instant::now() < deadline,
                "private keyring did not acquire the bus name"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

fn fixture_root() -> PathBuf {
    let root = PathBuf::from(std::env::var_os("HORIZON_CREDENTIAL_TEST_ROOT").expect("private fixture required"));
    assert!(root.is_absolute());
    assert!(
        root.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("horizon-browser-keyring.")
    );
    assert_eq!(
        std::env::var_os("XDG_DATA_HOME"),
        Some(root.join("data").into_os_string())
    );
    assert_eq!(
        std::env::var_os("XDG_RUNTIME_DIR"),
        Some(root.join("runtime").into_os_string())
    );
    assert_eq!(
        std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap(),
        std::env::var("HORIZON_CREDENTIAL_TEST_BUS").unwrap()
    );
    root
}

#[test]
#[ignore = "requires scripts/browser-smoke/credentials.sh private Secret Service fixture"]
fn credentials_survive_secret_service_restart() {
    let root = fixture_root();
    let daemon = Daemon::start(&root);
    let profile = basic_profile(CredentialStoreKind::OsKeychain, CredentialStoreKind::OsKeychain);
    let user = CredentialReference::from("user");
    let key = CredentialReference::from("key");
    let user_locator = CredentialLocator::new(&profile.endpoint, &user, &profile.credential_bindings[&user]);
    let key_locator = CredentialLocator::new(&profile.endpoint, &key, &profile.credential_bindings[&key]);
    let mut store = KeyringCredentialStore::open().unwrap();
    let mut workbench = CredentialWorkbench::spawn_platform();
    wait_until(&mut workbench, |w| w.keychain_state() == &KeychainState::Available);
    for (reference, value) in [
        (&user, b"synthetic-user".as_slice()),
        (&key, b"synthetic-key".as_slice()),
    ] {
        workbench.store_in_keychain("grid", &profile, reference, value).unwrap();
    }
    wait_until(&mut workbench, |w| !w.is_busy());
    assert!(workbench.take_notices().iter().all(|notice| notice.error.is_none()));
    assert!(store.contains(&user_locator).unwrap());
    assert!(store.contains(&key_locator).unwrap());

    drop(daemon);
    let _restarted = Daemon::start(&root);
    let mut capture = Capture(Vec::new());
    store
        .with_secret(&user_locator, &mut capture)
        .expect("read through the retained adapter after restart");
    assert_eq!(capture.0, b"synthetic-user");
    store
        .put(&key_locator, b"synthetic-replacement")
        .expect("save through the retained adapter after restart");
    store.with_secret(&key_locator, &mut capture).unwrap();
    assert_eq!(capture.0, b"synthetic-replacement");
    workbench
        .store_in_keychain("grid", &profile, &user, b"synthetic-replacement-user")
        .unwrap();
    wait_until(&mut workbench, |w| !w.is_busy());
    let notices = workbench.take_notices();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].error.is_none(), "{notices:?}");
    assert!(
        workbench
            .readiness(&profile)
            .iter()
            .all(|row| row.state == CredentialState::Present)
    );
    store.with_secret(&user_locator, &mut capture).unwrap();
    assert_eq!(capture.0, b"synthetic-replacement-user");
    store.delete(&user_locator).unwrap();
    store.delete(&key_locator).unwrap();
    assert!(!store.contains(&user_locator).unwrap());
    assert!(!store.contains(&key_locator).unwrap());
}

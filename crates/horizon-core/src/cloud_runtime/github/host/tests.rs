use super::*;

fn settings(app_id: u64) -> Settings {
    Settings {
        app_id,
        slug: "horizon-example".into(),
        client_id: "Iv23synthetic".into(),
        client_secret_file: PathBuf::from("/synthetic/secret"),
        mode: Mode::Ask,
    }
}

#[test]
fn each_app_has_its_own_sign_in_on_this_computer() {
    let root = Path::new("/synthetic/cloud");
    assert_eq!(
        path(root, &settings(42)),
        Path::new("/synthetic/cloud/credentials/github-host-42.json")
    );
    assert_ne!(path(root, &settings(42)), path(root, &settings(43)));
}

#[test]
fn without_a_sign_in_this_computer_asks_without_calling_github() {
    let root = tempfile::tempdir().unwrap();
    assert!(current(root.path(), &settings(42)).unwrap().is_none());
    assert!(forget(root.path(), &settings(42)).is_ok(), "forgetting nothing is fine");
}

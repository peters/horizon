#![cfg(unix)]

use horizon_app_provider::Error;
use horizon_app_provider::artifact::Artifact;
use horizon_app_testing::contract::{Contract, Platform};
use std::fs;

fn contract() -> Contract {
    Contract::from_agents("```yaml\nremote-device-testing:\n  version: 1\n  provider: browserstack\n  apps:\n    ios:\n      build: [build]\n      artifact: build/App.ipa\n      bundle_id: com.example.app\n  tunnel:\n    ports: {backend: 8080}\n  matrix: [{platform: ios, form: phone}]\n  recipes: [recipe.md]\n```").unwrap()
}

#[test]
fn declared_artifacts_are_captured_once_and_content_hashed() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("build")).unwrap();
    fs::write(root.path().join("build/App.ipa"), b"PK\x03\x04first").unwrap();
    let first = Artifact::capture(root.path(), &contract(), Platform::Ios).unwrap();
    fs::write(root.path().join("build/App.ipa"), b"PK\x03\x04other").unwrap();
    let second = Artifact::capture(root.path(), &contract(), Platform::Ios).unwrap();
    assert_ne!(first.sha256(), second.sha256());
    assert_eq!(first.bytes(), 9);
    assert_eq!(first.sha256().len(), 64);
    assert_eq!(
        Artifact::capture(root.path(), &contract(), Platform::Android).err(),
        Some(Error::ArtifactRejected)
    );
}

#[test]
fn missing_non_zip_and_directory_artifacts_fail_without_echoing_the_path() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("build")).unwrap();
    for value in [b"not-a-zip".as_slice(), b"".as_slice()] {
        fs::write(root.path().join("build/App.ipa"), value).unwrap();
        assert_eq!(
            Artifact::capture(root.path(), &contract(), Platform::Ios).err(),
            Some(Error::ArtifactRejected)
        );
    }
    fs::remove_file(root.path().join("build/App.ipa")).unwrap();
    fs::create_dir(root.path().join("build/App.ipa")).unwrap();
    let error = Artifact::capture(root.path(), &contract(), Platform::Ios)
        .err()
        .unwrap();
    assert!(!error.to_string().contains(root.path().to_str().unwrap()));
}

#[test]
fn artifact_and_ancestor_symlinks_are_rejected_even_inside_the_project() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("build")).unwrap();
    fs::write(root.path().join("actual.ipa"), b"PK\x03\x04data").unwrap();
    symlink(root.path().join("actual.ipa"), root.path().join("build/App.ipa")).unwrap();
    assert_eq!(
        Artifact::capture(root.path(), &contract(), Platform::Ios).err(),
        Some(Error::ArtifactRejected)
    );
    fs::remove_file(root.path().join("build/App.ipa")).unwrap();
    fs::remove_dir(root.path().join("build")).unwrap();
    fs::create_dir(root.path().join("other")).unwrap();
    fs::write(root.path().join("other/App.ipa"), b"PK\x03\x04data").unwrap();
    symlink(root.path().join("other"), root.path().join("build")).unwrap();
    assert_eq!(
        Artifact::capture(root.path(), &contract(), Platform::Ios).err(),
        Some(Error::ArtifactRejected)
    );
}

#[test]
fn fifo_artifacts_are_rejected_without_blocking() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("build")).unwrap();
    assert!(
        std::process::Command::new("/usr/bin/mkfifo")
            .args(["-m", "600"])
            .arg(root.path().join("build/App.ipa"))
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        Artifact::capture(root.path(), &contract(), Platform::Ios).err(),
        Some(Error::ArtifactRejected)
    );
}

#[test]
fn retained_directory_capture_cannot_follow_a_replaced_project_root() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("project");
    fs::create_dir_all(root.join("build")).unwrap();
    fs::write(root.join("build/App.ipa"), b"PK\x03\x04original").unwrap();
    let directory = fs::File::open(&root).unwrap();
    let original = Artifact::capture_directory(&directory, &contract(), Platform::Ios).unwrap();
    fs::rename(&root, outer.path().join("retained")).unwrap();
    fs::create_dir_all(root.join("build")).unwrap();
    fs::write(root.join("build/App.ipa"), b"PK\x03\x04replacement").unwrap();
    let pinned = Artifact::capture_directory(&directory, &contract(), Platform::Ios).unwrap();
    let replaced = Artifact::capture(&root, &contract(), Platform::Ios).unwrap();
    assert_eq!(pinned.sha256(), original.sha256());
    assert_ne!(pinned.sha256(), replaced.sha256());
    assert!(matches!(
        Artifact::capture_directory(
            &fs::File::open(root.join("build/App.ipa")).unwrap(),
            &contract(),
            Platform::Ios
        ),
        Err(Error::ArtifactRejected)
    ));
}

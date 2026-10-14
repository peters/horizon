use super::*;
use crate::test_egui::DiscardTextures;
use horizon_core::cloud_runtime::repository::launch::{Configuration, prepare_with_configuration};
use std::process::Command;

const CONFIG: &str = "version: 1\ndefault: built\nprofiles:\n  built:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n    build:\n      context: .horizon\n      dockerfile: .horizon/Dockerfile\n  plain:\n    provider: runpod\n    image: example.invalid/plain\n    cpu: 4\n    memory_gb: 8\n";

fn git(repository: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid"])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn write(repository: &Path, path: &str, text: &str) {
    std::fs::write(repository.join(path), text).unwrap();
}

/// A checkout with a committed recipe in `.horizon` and the commit it is on.
fn checkout() -> (tempfile::TempDir, String) {
    let repository = tempfile::tempdir().unwrap();
    let path = repository.path();
    git(path, &["init", "--quiet"]);
    std::fs::create_dir(path.join(".horizon")).unwrap();
    write(path, ".horizon/cloud.yml", CONFIG);
    write(path, ".horizon/Dockerfile", "FROM scratch\n");
    git(path, &["add", "."]);
    git(path, &["commit", "--quiet", "-m", "Recipe"]);
    let revision = git(path, &["rev-parse", "HEAD"]);
    (repository, revision)
}

/// What the dialog reads for `revision`, empty for the checked-out commit.
fn dialog(repository: &Path, revision: &str) -> Production {
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let prepared = prepare_with_configuration(
        repository.to_str().unwrap(),
        revision,
        Configuration::WorkingCopy,
        &runner,
    )
    .unwrap();
    let mut form = Production {
        selected_profile: "built".into(),
        ..Production::default()
    };
    form.launch.uncommitted = read(&prepared, &cancel);
    form.profiles = Some(prepared.config);
    form
}

fn built(form: &Production) -> Option<&Build> {
    form.profiles.as_ref()?.profiles["built"].build.as_ref()
}

fn painted(form: &Production) -> String {
    let ctx = egui::Context::default();
    ctx.run_ui(egui::RawInput::default(), |ui| section(ui, form))
        .discard_textures()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_dialog_lists_the_uncommitted_files_of_the_chosen_build_and_refuses_a_secret_one() {
    let (repository, revision) = checkout();
    let path = repository.path();
    let form = dialog(path, "");
    assert_eq!(
        form.launch.uncommitted.keys().collect::<Vec<_>>(),
        ["built"],
        "only a build reads files"
    );
    assert!(painted(&form).contains("No uncommitted file goes into the image"));
    write(path, ".horizon/Dockerfile", "FROM scratch\nCOPY tool.sh /\n");
    write(path, ".horizon/tool.sh", "echo uncommitted\n");
    let form = dialog(path, "");
    let shown = painted(&form);
    assert!(shown.contains("check that none of them holds a secret"), "{shown}");
    assert!(shown.contains("• modified .horizon/Dockerfile"), "{shown}");
    assert!(shown.contains("• new .horizon/tool.sh"), "{shown}");
    assert_eq!(blocked(&form), None);
    let (_, confirmed_files) = confirmed(&form, built(&form)).unwrap().unwrap();
    let listed = form.launch.uncommitted["built"].as_ref().unwrap().hash();
    assert!(listed.is_some());
    assert_eq!(confirmed_files.hash, listed);
    // The image-only profile builds nothing, so it lists and confirms nothing.
    let plain = Production {
        selected_profile: "plain".into(),
        ..dialog(path, "")
    };
    assert!(painted(&plain).is_empty());
    assert!(confirmed(&plain, None).unwrap().is_none());
    // A secret-looking name is refused under the profile and holds back Start.
    write(path, ".horizon/.env", "TOKEN=synthetic\n");
    let form = dialog(path, "");
    let shown = painted(&form);
    assert!(
        shown.contains(".horizon/.env") && shown.contains("common secret file"),
        "{shown}"
    );
    assert!(!shown.contains("synthetic"), "{shown}");
    assert_eq!(super::super::storage::launch_reason(&form), Some(REFUSED));
    assert!(confirmed(&form, built(&form)).is_err());
    // A typed revision that is not checked out builds that commit alone.
    std::fs::remove_file(path.join(".horizon/.env")).unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "--quiet", "-m", "Tool"]);
    let earlier = dialog(path, &revision);
    assert!(earlier.launch.uncommitted.is_empty());
    assert!(confirmed(&earlier, built(&earlier)).unwrap().is_none());
}

#[test]
fn start_refuses_files_or_a_checkout_that_changed_since_the_list() {
    let (repository, revision) = checkout();
    let path = repository.path();
    let cancel = Cancellation::default();
    write(path, ".horizon/tool.sh", "echo listed\n");
    let form = dialog(path, "");
    let listed = confirmed(&form, built(&form)).unwrap().unwrap();
    unchanged(path, &revision, &listed, &cancel).unwrap();
    write(path, ".horizon/tool.sh", "echo edited after the list\n");
    let error = unchanged(path, &revision, &listed, &cancel).unwrap_err();
    assert!(matches!(error, cloud_runtime::Error::Invalid(message) if message == CHANGED));
    write(path, ".horizon/tool.sh", "echo listed\n");
    unchanged(path, &revision, &listed, &cancel).unwrap();
    // Nothing listed: a file added later is refused too.
    std::fs::remove_file(path.join(".horizon/tool.sh")).unwrap();
    let empty = dialog(path, "");
    let nothing = confirmed(&empty, built(&empty)).unwrap().unwrap();
    assert_eq!(nothing.1.hash, None);
    write(path, ".horizon/late.sh", "echo late\n");
    assert!(unchanged(path, &revision, &nothing, &cancel).is_err());
    git(path, &["add", "."]);
    git(path, &["commit", "--quiet", "-m", "Moved on"]);
    let error = unchanged(path, &revision, &nothing, &cancel).unwrap_err();
    assert!(matches!(error, cloud_runtime::Error::Invalid(message) if message == MOVED));
}

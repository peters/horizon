use super::super::{
    Configuration, MISSING_CONFIG, Prepared, is_missing_config,
    tests::{CONFIG, git, read as committed, repository},
};
use super::*;

const BUILD: &str = "    build:\n      context: .\n      dockerfile: Dockerfile\n";

fn load(path: &Path) -> super::super::super::Result<Prepared> {
    load_revision(path, "")
}

fn load_revision(path: &Path, revision: &str) -> super::super::super::Result<Prepared> {
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    super::super::prepare_with_configuration(path.to_str().unwrap(), revision, Configuration::WorkingCopy, &runner)
}

const PATH: &str = ".horizon/cloud.yml";

fn write(path: &Path, yaml: &str) {
    std::fs::write(path.join(PATH), yaml).unwrap();
}

fn commit_all(path: &Path) {
    git(path, &["add", "-A"]);
    git(
        path,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "Update settings",
        ],
    );
}

#[test]
fn uncommitted_sizes_apply_to_the_checked_out_commit_only() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    let first = committed(temp.path()).unwrap().revision;
    write(temp.path(), &CONFIG.replace("cpu: 4", "min_cpu: 8"));
    let prepared = load(temp.path()).unwrap();
    assert_eq!(prepared.revision, first, "the source stays the commit");
    assert_eq!(prepared.config.profiles["dev"].cpu, 8);
    assert!(prepared.notes.is_empty(), "{:?}", prepared.notes);
    assert_eq!(
        committed(temp.path()).unwrap().config.profiles["dev"].cpu,
        4,
        "other callers read the commit alone"
    );
    commit_all(temp.path());
    write(temp.path(), &CONFIG.replace("cpu: 4", "cpu: 16"));
    assert_eq!(load(temp.path()).unwrap().config.profiles["dev"].cpu, 16);
    // An earlier commit is not checked out, so its own file applies unchanged.
    let earlier = load_revision(temp.path(), &first).unwrap();
    assert_eq!(earlier.config.profiles["dev"].cpu, 4);
    assert!(earlier.notes.is_empty());
    // The checked-out commit named by its ID reads the working copy as HEAD does.
    let head = committed(temp.path()).unwrap().revision;
    assert_eq!(
        load_revision(temp.path(), &head).unwrap().config.profiles["dev"].cpu,
        16
    );
}

#[test]
fn a_missing_or_invalid_working_copy_falls_back_to_the_commit_and_says_so() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    std::fs::remove_file(temp.path().join(PATH)).unwrap();
    let prepared = load(temp.path()).unwrap();
    assert_eq!(prepared.config.profiles["dev"].cpu, 4);
    assert_eq!(prepared.notes, [MISSING]);
    for yaml in [
        "invalid-private-marker".to_owned(),
        format!("{CONFIG}    api_key: private-marker\n"),
        "#".repeat(usize::try_from(MAX_CONFIG_BYTES).unwrap() + 1),
    ] {
        write(temp.path(), &yaml);
        let prepared = load(temp.path()).unwrap();
        assert_eq!(prepared.config.profiles["dev"].cpu, 4);
        assert_eq!(prepared.notes, [INVALID]);
    }
}

#[cfg(unix)]
#[test]
fn a_linked_working_copy_is_not_read() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), CONFIG.replace("cpu: 4", "cpu: 32")).unwrap();
    std::fs::remove_file(temp.path().join(PATH)).unwrap();
    std::os::unix::fs::symlink(outside.path(), temp.path().join(PATH)).unwrap();
    let prepared = load(temp.path()).unwrap();
    assert_eq!(prepared.config.profiles["dev"].cpu, 4);
    assert_eq!(prepared.notes, [INVALID]);
    // A linked folder is not followed either, and a pipe is not waited on.
    std::fs::remove_file(temp.path().join(PATH)).unwrap();
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("cloud.yml"), CONFIG.replace("cpu: 4", "cpu: 32")).unwrap();
    std::fs::remove_dir(temp.path().join(".horizon")).unwrap();
    std::os::unix::fs::symlink(folder.path(), temp.path().join(".horizon")).unwrap();
    assert_eq!(load(temp.path()).unwrap().notes, [INVALID]);
    std::fs::remove_file(temp.path().join(".horizon")).unwrap();
    std::fs::create_dir(temp.path().join(".horizon")).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(temp.path().join(PATH))
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(load(temp.path()).unwrap().notes, [INVALID], "a pipe is refused at once");
}

#[test]
fn a_commit_without_settings_offers_the_working_copy_image_profiles() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    std::fs::remove_file(temp.path().join(PATH)).unwrap();
    commit_all(temp.path());
    assert!(is_missing_config(&load(temp.path()).err().unwrap()));
    write(temp.path(), "invalid-private-marker");
    let error = load(temp.path()).err().unwrap().to_string();
    assert!(error.contains("Invalid .horizon/cloud.yml") && !error.contains("private-marker"));
    // Untracked settings: the image-only profile is offered, the build profile is not.
    write(
        temp.path(),
        &format!(
            "{}{BUILD}  image-only:\n    provider: runpod\n    image: example.invalid/prebuilt\n    cpu: 2\n    memory_gb: 4\n",
            CONFIG
                .replace("dev:", "built:")
                .replace("default: dev", "default: built")
        ),
    );
    let prepared = load(temp.path()).unwrap();
    assert_eq!(prepared.config.profiles.keys().collect::<Vec<_>>(), ["image-only"]);
    assert_eq!(
        prepared.config.default, "image-only",
        "the build default is not offered"
    );
    assert_eq!(
        prepared.notes,
        ["Profile built builds an image, so New cloud offers it after you commit it."]
    );
    // Only build profiles: nothing can start before a commit.
    write(temp.path(), &format!("{CONFIG}{BUILD}"));
    let error = load(temp.path()).err().unwrap();
    assert!(is_missing_config(&error), "{error}");
    assert!(MISSING_CONFIG.contains("after you commit it"));
}

#[test]
fn a_build_profile_takes_what_is_built_from_the_commit_and_its_size_from_the_working_copy() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    let committed_yaml = format!("{CONFIG}{BUILD}    capabilities:\n      agents: [codex]\n");
    write(temp.path(), &committed_yaml);
    commit_all(temp.path());
    let edited = committed_yaml
        .replace("cpu: 4", "cpu: 16")
        .replace("dockerfile: Dockerfile", "dockerfile: Other.Dockerfile")
        .replace("example.invalid/worker", "example.invalid/elsewhere")
        .replace("agents: [codex]", "agents: [claude]");
    write(
        temp.path(),
        &format!("{edited}    idle_stop_minutes: 30\nsource:\n  submodule_history: pinned\n"),
    );
    let prepared = load(temp.path()).unwrap();
    let profile = &prepared.config.profiles["dev"];
    assert_eq!(profile.cpu, 16);
    assert_eq!(profile.idle_stop_minutes, Some(30));
    assert_eq!(profile.build.as_ref().unwrap().dockerfile, "Dockerfile");
    assert_eq!(profile.image, "example.invalid/worker");
    assert_eq!(profile.capabilities.agents, [horizon_cloud::Agent::Codex].into());
    assert!(prepared.config.source.is_default());
    assert_eq!(
        prepared.notes,
        ["Uncommitted changes to build, image, capabilities and source are not used until you commit them."]
    );
    // A working copy that drops the recipe still builds as committed.
    write(
        temp.path(),
        &CONFIG.replace("example.invalid/worker", "example.invalid/prebuilt"),
    );
    let prepared = load(temp.path()).unwrap();
    assert!(prepared.config.profiles["dev"].build.is_some());
    assert_eq!(prepared.config.profiles["dev"].image, "example.invalid/worker");
    assert_eq!(
        prepared.notes,
        ["Uncommitted changes to build, image and capabilities are not used until you commit them."]
    );
}

#[test]
fn image_only_profiles_take_image_and_capabilities_from_the_working_copy() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    write(
        temp.path(),
        &format!(
            "{}    gpu: true\n    capabilities:\n      agents: []\n      browsers: []\n      desktop: true\n",
            CONFIG.replace("example.invalid/worker", "example.invalid/test-worker")
        ),
    );
    let prepared = load(temp.path()).unwrap();
    let profile = &prepared.config.profiles["dev"];
    assert_eq!(profile.image, "example.invalid/test-worker");
    assert!(profile.gpu && profile.capabilities.desktop && profile.capabilities.agents.is_empty());
    assert!(prepared.notes.is_empty());
}

#[test]
fn companions_and_the_profile_set_follow_their_own_rules() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    write(
        temp.path(),
        &format!(
            "{}  second:\n    provider: runpod\n    image: example.invalid/second\n    cpu: 2\n    memory_gb: 4\ncompanions:\n  extra:\n    repository: example/service\n    profile: dev\n",
            CONFIG.replace("default: dev", "default: second")
        ),
    );
    let prepared = load(temp.path()).unwrap();
    assert_eq!(prepared.config.default, "second");
    assert_eq!(prepared.config.profiles.keys().collect::<Vec<_>>(), ["dev", "second"]);
    assert!(prepared.config.companions.is_empty());
    assert_eq!(
        prepared.notes,
        ["Uncommitted changes to companions are not used until you commit them."]
    );
    // A working copy can drop a committed profile.
    write(
        temp.path(),
        &CONFIG
            .replace("dev:", "renamed:")
            .replace("default: dev", "default: renamed"),
    );
    let prepared = load(temp.path()).unwrap();
    assert_eq!(prepared.config.profiles.keys().collect::<Vec<_>>(), ["renamed"]);
}

#[test]
fn working_copy_changes_that_break_the_committed_recipe_leave_the_commit_in_place() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    write(
        temp.path(),
        &format!("{CONFIG}{BUILD}    capabilities:\n      browserstack: {{}}\n"),
    );
    commit_all(temp.path());
    let hetzner = format!("{}{BUILD}", CONFIG.replace("provider: runpod", "provider: hetzner"));
    write(temp.path(), &hetzner);
    let prepared = load(temp.path()).unwrap();
    assert_eq!(prepared.config.profiles["dev"].provider, "runpod");
    assert_eq!(prepared.notes, [MISMATCH]);
    // Every working-copy profile builds and is new: the committed settings apply.
    write(
        temp.path(),
        &format!(
            "{}{BUILD}",
            CONFIG.replace("dev:", "new:").replace("default: dev", "default: new")
        ),
    );
    let prepared = load(temp.path()).unwrap();
    assert_eq!(prepared.config.profiles.keys().collect::<Vec<_>>(), ["dev"]);
    assert_eq!(
        prepared.notes,
        [
            "Profile new builds an image, so New cloud offers it after you commit it.",
            NONE_OFFERED
        ]
    );
}

#[test]
fn the_fingerprint_follows_the_file_and_the_checked_out_commit() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    let cancel = Cancellation::default();
    let directory = temp.path().to_str().unwrap();
    let first = fingerprint(directory, &cancel);
    assert_ne!(first, Fingerprint::default());
    assert_eq!(fingerprint(directory, &cancel), first, "nothing changed");
    write(temp.path(), &CONFIG.replace("cpu: 4", "cpu: 8"));
    let edited = fingerprint(directory, &cancel);
    assert_ne!(edited, first);
    commit_all(temp.path());
    let committed = fingerprint(directory, &cancel);
    assert_ne!(committed, edited, "the same file on a new commit");
    std::fs::remove_file(temp.path().join(PATH)).unwrap();
    assert_ne!(fingerprint(directory, &cancel), committed);
    let outside = tempfile::tempdir().unwrap();
    assert_eq!(
        fingerprint(outside.path().to_str().unwrap(), &cancel),
        Fingerprint::default()
    );
}

#[test]
fn names_are_listed_as_a_sentence() {
    assert_eq!(list(&["a"]), "a");
    assert_eq!(list(&["a", "b"]), "a and b");
    assert_eq!(list(&["a", "b", "c"]), "a, b and c");
    assert_eq!(
        unoffered_note(&["a".into(), "b".into()]),
        "Profiles a and b build images, so New cloud offers them after you commit them."
    );
}

#[cfg(unix)]
#[test]
fn the_fingerprint_reads_a_checkout_whose_folder_name_holds_a_newline() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("two\nlines");
    std::fs::create_dir(&path).unwrap();
    repository(&path);
    let cancel = Cancellation::default();
    let directory = path.to_str().unwrap();
    let first = fingerprint(directory, &cancel);
    write(&path, &CONFIG.replace("cpu: 4", "cpu: 8"));
    assert_ne!(
        fingerprint(directory, &cancel),
        first,
        "the file of that folder is read"
    );
}

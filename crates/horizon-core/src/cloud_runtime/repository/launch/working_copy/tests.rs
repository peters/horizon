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
fn a_commit_without_settings_offers_every_working_copy_profile() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    std::fs::remove_file(temp.path().join(PATH)).unwrap();
    commit_all(temp.path());
    assert!(is_missing_config(&load(temp.path()).err().unwrap()));
    write(temp.path(), "invalid-private-marker");
    let error = load(temp.path()).err().unwrap().to_string();
    assert!(error.contains("Invalid .horizon/cloud.yml") && !error.contains("private-marker"));
    // Untracked settings: the image builds from the working copy, so a build profile is
    // offered as well as an image-only one.
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
    assert!(prepared.working_copy);
    assert_eq!(
        prepared.config.profiles.keys().collect::<Vec<_>>(),
        ["built", "image-only"]
    );
    assert_eq!(prepared.config.default, "built");
    assert!(prepared.notes.is_empty(), "{:?}", prepared.notes);
    assert!(!MISSING_CONFIG.contains("commit it"));
}

#[test]
fn a_build_profile_takes_everything_but_the_source_from_the_working_copy_on_the_checked_out_commit() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    let committed_yaml = format!("{CONFIG}{BUILD}    capabilities:\n      agents: [codex]\n");
    write(temp.path(), &committed_yaml);
    commit_all(temp.path());
    let base = committed(temp.path()).unwrap().revision;
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
    assert!(prepared.working_copy);
    let profile = &prepared.config.profiles["dev"];
    assert_eq!(profile.cpu, 16);
    assert_eq!(profile.idle_stop_minutes, Some(30));
    assert_eq!(profile.build.as_ref().unwrap().dockerfile, "Other.Dockerfile");
    assert_eq!(profile.image, "example.invalid/elsewhere");
    assert_eq!(profile.capabilities.agents, [horizon_cloud::Agent::Claude].into());
    assert!(prepared.config.source.is_default(), "the source is a committed archive");
    assert_eq!(
        prepared.notes,
        [
            "Uncommitted changes to source are not used until you commit them.".to_owned(),
            super::super::idle_period_note("dev", profile).unwrap(),
        ]
    );
    // A working copy that drops the recipe runs its image without a build.
    write(
        temp.path(),
        &CONFIG.replace("example.invalid/worker", "example.invalid/prebuilt"),
    );
    let prepared = load(temp.path()).unwrap();
    assert_eq!(prepared.config.profiles["dev"].build, None);
    assert_eq!(prepared.config.profiles["dev"].image, "example.invalid/prebuilt");
    // A typed revision that is not checked out reads that commit alone.
    write(temp.path(), &format!("{edited}    idle_stop_minutes: 30\n"));
    commit_all(temp.path());
    let earlier = load_revision(temp.path(), &base).unwrap();
    assert!(!earlier.working_copy);
    let profile = &earlier.config.profiles["dev"];
    assert_eq!(profile.build.as_ref().unwrap().dockerfile, "Dockerfile");
    assert_eq!(profile.capabilities.agents, [horizon_cloud::Agent::Codex].into());
    assert!(earlier.notes.is_empty());
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

#[cfg(windows)]
#[test]
fn a_junction_in_place_of_the_folder_is_not_followed() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("cloud.yml"), CONFIG.replace("cpu: 4", "cpu: 32")).unwrap();
    std::fs::remove_dir_all(temp.path().join(".horizon")).unwrap();
    assert!(
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(temp.path().join(".horizon"))
            .arg(outside.path())
            .status()
            .unwrap()
            .success()
    );
    let prepared = load(temp.path()).unwrap();
    assert_eq!(prepared.config.profiles["dev"].cpu, 4);
    assert_eq!(prepared.notes, [INVALID]);
}

#[test]
fn a_rebuild_reads_the_working_copy_whole_but_source_and_companions_from_the_commit() {
    let temp = tempfile::tempdir().unwrap();
    repository(temp.path());
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let committed = || committed(temp.path()).unwrap().config;
    // The recipe and the capabilities of a profile with a build apply too.
    write(
        temp.path(),
        &format!(
            "{CONFIG}{BUILD}    capabilities:\n      agents: [codex]\ncompanions:\n  extra:\n    repository: example/service\n    profile: dev\n"
        ),
    );
    let (config, notes) = for_rebuild(temp.path(), Some(committed()), &runner).unwrap();
    let profile = &config.unwrap().profiles["dev"];
    assert_eq!(profile.build.as_ref().unwrap().dockerfile, "Dockerfile");
    assert!(profile.capabilities.agents.contains(&horizon_cloud::Agent::Codex));
    assert_eq!(
        notes,
        ["Uncommitted changes to companions are not used until you commit them."]
    );
    write(temp.path(), "not: yaml: [");
    let (config, notes) = for_rebuild(temp.path(), Some(committed()), &runner).unwrap();
    assert_eq!(config.unwrap().profiles["dev"].build, None, "the commit's file");
    assert_eq!(notes, [INVALID]);
    std::fs::remove_file(temp.path().join(PATH)).unwrap();
    assert_eq!(
        for_rebuild(temp.path(), Some(committed()), &runner).unwrap().1,
        [MISSING]
    );
    assert!(for_rebuild(temp.path(), None, &runner).unwrap().0.is_none());
}

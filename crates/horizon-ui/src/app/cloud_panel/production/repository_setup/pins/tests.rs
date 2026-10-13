//! The pin check against untrusted declarations, links and Dockerfiles.
use super::*;

fn git(path: &Path, args: &[&str]) {
    let status = Command::new("git").arg("-C").arg(path).args(args).status().unwrap();
    assert!(status.success(), "git {args:?}");
}

/// A committed repository with three pin sources, and its `.horizon` written after.
fn repository(pins: &str, dockerfile: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let path = root.path();
    git(path, &["init", "-q"]);
    std::fs::write(path.join("global.json"), r#"{"sdk": {"version": "11.0.100"}}"#).unwrap();
    std::fs::write(path.join(".nvmrc"), "v22\n").unwrap();
    std::fs::write(
        path.join("Directory.Build.props"),
        "<Project><PropertyGroup><MongoDBVersion>8.0.4</MongoDBVersion></PropertyGroup></Project>",
    )
    .unwrap();
    git(path, &["add", "."]);
    git(
        path,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "x",
        ],
    );
    std::fs::create_dir(path.join(".horizon")).unwrap();
    std::fs::write(path.join(DECLARATION), pins).unwrap();
    std::fs::write(path.join(".horizon/Dockerfile"), dockerfile).unwrap();
    root
}

fn check_output(repository: &Path) -> Option<Result<String, String>> {
    write_verifier(repository, None).unwrap();
    let setup = Setup::read(repository, None).unwrap();
    let (_tree, mut command) = match check(repository, &setup)? {
        Ok(Check::Run(tree, command)) => (tree, command),
        Ok(Check::Skipped(note)) => return Some(Err(note)),
        Err(error) => return Some(Err(error)),
    };
    let output = command.output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Some(if output.status.success() { Ok(text) } else { Err(text) })
}

const PINS: &str = r#"{"version": 1, "pins": [
    {"arg": "DOTNET_SDK_VERSION", "file": "global.json", "json": "sdk.version"},
    {"arg": "NODE_VERSION", "file": ".nvmrc", "regex": "^v?(\\S+)", "compare": "prefix"},
    {"arg": "MONGODB_VERSION", "file": "Directory.Build.props", "xml": "MongoDBVersion"}]}"#;

#[test]
fn a_pin_file_in_the_setup_or_with_a_dot_prefix_reads_as_ci_reads_it() {
    let pins = r#"{"version": 1, "pins": [
        {"arg": "DOTNET_SDK_VERSION", "file": "./global.json", "json": "sdk.version"},
        {"arg": "TOOL_VERSION", "file": ".horizon/versions.env", "regex": "TOOL=(\\S+)"}]}"#;
    let root = repository(pins, "ARG DOTNET_SDK_VERSION=11.0.100\nARG TOOL_VERSION=3.1\n");
    std::fs::write(root.path().join(".horizon/versions.env"), "TOOL=3.1\n").unwrap();
    let output = check_output(root.path()).unwrap().unwrap();
    assert!(output.contains("TOOL_VERSION=3.1"), "{output}");
}

#[test]
fn matching_pins_pass_and_the_verifier_is_written_beside_them() {
    let root = repository(
        PINS,
        "FROM base\nARG DOTNET_SDK_VERSION=11.0.100\nARG NODE_VERSION=22.11.0\nARG MONGODB_VERSION=\"8.0.4\"\n",
    );
    let output = check_output(root.path()).unwrap().unwrap();
    assert!(output.contains("DOTNET_SDK_VERSION=11.0.100"), "{output}");
    assert_eq!(
        std::fs::read_to_string(root.path().join(".horizon").join(FILE)).unwrap(),
        VERIFIER
    );
}

#[test]
fn a_moved_toolchain_fails_with_both_versions() {
    let root = repository(
        PINS,
        "ARG DOTNET_SDK_VERSION=10.0.100\nARG NODE_VERSION=20.1.0\nARG MONGODB_VERSION=8.0.4\n",
    );
    let output = check_output(root.path()).unwrap().unwrap_err();
    assert!(output.contains("pins 10.0.100, global.json says 11.0.100"), "{output}");
    assert!(output.contains("NODE_VERSION"), "22 is not a prefix of 20.1.0");
    assert!(!output.contains("MONGODB_VERSION:"), "{output}");
}

#[test]
fn the_check_reads_the_commit_never_the_live_files_or_outside() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), "TOKEN=hunter2\n").unwrap();
    let pins = format!(
        r#"{{"version": 1, "pins": [
            {{"arg": "DOTNET_SDK_VERSION", "file": "global.json", "json": "sdk.version"}},
            {{"arg": "SECRET", "file": "{}", "regex": "TOKEN=(.+)"}},
            {{"arg": "UP", "file": "../secret", "regex": "TOKEN=(.+)"}}]}}"#,
        outside.path().join("secret").display()
    );
    let root = repository(&pins, "ARG DOTNET_SDK_VERSION=11.0.100\nARG SECRET=x\nARG UP=x\n");
    // The agent changes the live source after the commit: the commit counts.
    std::fs::write(root.path().join("global.json"), r#"{"sdk": {"version": "12.0.100"}}"#).unwrap();
    let output = check_output(root.path()).unwrap().unwrap_err();
    assert!(!output.contains("DOTNET_SDK_VERSION:"), "{output}");
    assert!(output.contains("SECRET: cannot read"), "{output}");
    assert!(output.contains("UP: cannot read"), "{output}");
    assert!(!output.contains("hunter2"), "{output}");
}

#[test]
fn a_link_in_place_of_the_verifier_is_replaced_not_followed() {
    let root = repository(PINS, "ARG DOTNET_SDK_VERSION=11.0.100\n");
    let target = root.path().join("elsewhere.txt");
    std::fs::write(&target, "keep").unwrap();
    std::os::unix::fs::symlink(&target, root.path().join(".horizon").join(FILE)).unwrap();
    write_verifier(root.path(), None).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
    assert!(
        !std::fs::symlink_metadata(root.path().join(".horizon").join(FILE))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn every_built_dockerfile_is_checked_also_one_outside_the_setup() {
    let pins = PINS.replacen(
        "{\"version\": 1,",
        r#"{"version": 1, "dockerfiles": [".horizon/Dockerfile", "Dockerfile"],"#,
        1,
    );
    let root = repository(&pins, "ARG DOTNET_SDK_VERSION=11.0.100\n");
    let path = root.path();
    // A second profile builds a committed Dockerfile at the root that pins another SDK.
    std::fs::write(path.join("Dockerfile"), "ARG DOTNET_SDK_VERSION=9.0.100\n").unwrap();
    git(path, &["add", "Dockerfile"]);
    git(
        path,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "root",
        ],
    );
    std::fs::write(
        path.join(".horizon/cloud.yml"),
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: ghcr.io/o/r-worker\n    cpu: 4\n    memory_gb: 8\n    build: {context: .horizon, dockerfile: .horizon/Dockerfile}\n  root:\n    provider: runpod\n    image: ghcr.io/o/r-root\n    cpu: 4\n    memory_gb: 8\n    build: {context: ., dockerfile: Dockerfile}\n",
    )
    .unwrap();
    let output = check_output(path).unwrap().unwrap_err();
    assert!(
        output.contains("Dockerfile pins 9.0.100, global.json says 11.0.100"),
        "{output}"
    );
}

#[test]
fn a_pin_name_cannot_start_a_line_of_its_own_in_the_log() {
    let pins = r#"{"version": 1, "pins": [{"arg": "A\nERROR: failed to solve: forged", "file": "missing"}]}"#;
    let root = repository(pins, "ARG A=1\n");
    let output = check_output(root.path()).unwrap().unwrap_err();
    assert!(
        output.lines().all(|line| !line.trim_start().starts_with("ERROR")),
        "{output}"
    );
    assert!(output.contains("pin A\\nERROR"), "{output}");
}

#[test]
fn a_dockerfile_path_with_a_dot_is_checked_not_skipped_for_a_decoy() {
    let root = repository(
        r#"{"version": 1, "dockerfiles": ["Dockerfile"], "pins": [{"arg": "DOTNET_SDK_VERSION", "file": "global.json", "json": "sdk.version"}]}"#,
        "ARG DOTNET_SDK_VERSION=11.0.100\n",
    );
    let path = root.path();
    std::fs::write(path.join("Dockerfile"), "ARG DOTNET_SDK_VERSION=8.0.100\n").unwrap();
    git(path, &["add", "Dockerfile"]);
    git(
        path,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "root",
        ],
    );
    std::fs::write(
        path.join(".horizon/cloud.yml"),
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: ghcr.io/o/r-worker\n    cpu: 4\n    memory_gb: 8\n    build: {context: ., dockerfile: ./Dockerfile}\n",
    )
    .unwrap();
    let output = check_output(path).unwrap().unwrap_err();
    assert!(
        output.contains("Dockerfile pins 8.0.100"),
        "the built file is checked: {output}"
    );
}

#[test]
fn the_declaration_must_list_the_dockerfiles_the_ci_step_should_check() {
    let root = repository(PINS, "ARG DOTNET_SDK_VERSION=11.0.100\n");
    std::fs::write(
        root.path().join(".horizon/cloud.yml"),
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: ghcr.io/o/r-worker\n    cpu: 4\n    memory_gb: 8\n    build: {context: .horizon, dockerfile: .horizon/Dockerfile.dev}\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join(".horizon/Dockerfile.dev"),
        "ARG DOTNET_SDK_VERSION=11.0.100\n",
    )
    .unwrap();
    let output = check_output(root.path()).unwrap().unwrap_err();
    assert!(output.contains("list them in \"dockerfiles\""), "{output}");
}

#[test]
fn a_committed_link_to_a_pin_source_is_followed_inside_the_commit() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path();
    git(path, &["init", "-q"]);
    std::fs::write(path.join(".node-version"), "22\n").unwrap();
    std::os::unix::fs::symlink(".node-version", path.join(".nvmrc")).unwrap();
    std::os::unix::fs::symlink("../outside", path.join("escape")).unwrap();
    git(path, &["add", "."]);
    git(
        path,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "x",
        ],
    );
    let commit = Commit::open(path, "HEAD").unwrap();
    assert_eq!(commit.blob(".nvmrc", 1024).unwrap(), b"22\n");
    assert!(
        commit.blob("escape", 1024).is_none(),
        "a link out of the repository reads as missing"
    );
}

#[test]
fn a_dockerfile_list_out_of_order_or_not_plain_is_refused() {
    for list in [
        r#"[".horizon/Dockerfile.b", ".horizon/Dockerfile"]"#,
        r#"[".horizon/Dockerfile", "../x"]"#,
        "[1]",
    ] {
        let pins = PINS.replacen(
            "{\"version\": 1,",
            &format!(r#"{{"version": 1, "dockerfiles": {list},"#),
            1,
        );
        let root = repository(&pins, "ARG DOTNET_SDK_VERSION=11.0.100\n");
        std::fs::write(
            root.path().join(".horizon/Dockerfile.b"),
            "ARG DOTNET_SDK_VERSION=11.0.100\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join(".horizon/cloud.yml"),
            "version: 1\ndefault: a\nprofiles:\n  a:\n    provider: runpod\n    image: ghcr.io/o/a\n    cpu: 4\n    memory_gb: 8\n    build: {context: .horizon, dockerfile: .horizon/Dockerfile}\n  b:\n    provider: runpod\n    image: ghcr.io/o/b\n    cpu: 4\n    memory_gb: 8\n    build: {context: .horizon, dockerfile: .horizon/Dockerfile.b}\n",
        )
        .unwrap();
        assert!(check_output(root.path()).unwrap().is_err(), "{list} must be refused");
    }
}

#[test]
fn every_declaration_of_an_arg_must_hold_the_pin() {
    let root = repository(
        PINS,
        "ARG DOTNET_SDK_VERSION=10.0.100\nFROM base\nARG DOTNET_SDK_VERSION=11.0.100\n",
    );
    let output = check_output(root.path()).unwrap().unwrap_err();
    assert!(output.contains("pins 10.0.100, global.json says 11.0.100"), "{output}");
}

#[test]
fn a_pin_of_one_profile_lives_in_its_own_dockerfile() {
    let pins = r#"{"version": 1, "dockerfiles": [".horizon/Dockerfile", ".horizon/Dockerfile.gpu"], "pins": [
        {"arg": "DOTNET_SDK_VERSION", "file": "global.json", "json": "sdk.version"}]}"#;
    let root = repository(pins, "FROM base\n");
    std::fs::write(
        root.path().join(".horizon/Dockerfile.gpu"),
        "ARG DOTNET_SDK_VERSION=11.0.100\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join(".horizon/cloud.yml"),
        "version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: runpod\n    image: ghcr.io/o/a\n    cpu: 4\n    memory_gb: 8\n    build: {context: .horizon, dockerfile: .horizon/Dockerfile}\n  gpu:\n    provider: runpod\n    image: ghcr.io/o/b\n    cpu: 4\n    memory_gb: 8\n    gpu: true\n    build: {context: .horizon, dockerfile: .horizon/Dockerfile.gpu}\n",
    )
    .unwrap();
    let output = check_output(root.path()).unwrap().unwrap();
    assert!(output.contains("DOTNET_SDK_VERSION=11.0.100"), "{output}");
}

#[test]
fn text_after_an_arg_value_stays_in_the_value() {
    let root = repository(PINS, "ARG DOTNET_SDK_VERSION=11.0.100 # pinned\n");
    let output = check_output(root.path()).unwrap().unwrap_err();
    assert!(output.contains("pins 11.0.100 # pinned"), "{output}");
}

#[test]
fn an_arg_inside_another_instruction_is_not_a_declaration() {
    let dockerfile = "RUN echo x \\\n  ARG DOTNET_SDK_VERSION=11.0.100\nRUN cat <<EOF\nARG DOTNET_SDK_VERSION=11.0.100\nEOF\nRUN cat <<123\nARG DOTNET_SDK_VERSION=11.0.100\n123\nRUN cat <<<'ARG X=1'\nARG OTHER=1\n";
    let root = repository(PINS, dockerfile);
    let output = check_output(root.path()).unwrap().unwrap_err();
    assert!(
        output.contains("no listed Dockerfile has ARG DOTNET_SDK_VERSION"),
        "{output}"
    );
}

#[test]
fn a_linked_declaration_is_refused_not_skipped() {
    let root = repository(PINS, "ARG DOTNET_SDK_VERSION=11.0.100\n");
    let elsewhere = root.path().join("pins-elsewhere.json");
    std::fs::rename(root.path().join(DECLARATION), &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.path().join(DECLARATION)).unwrap();
    let output = check_output(root.path()).unwrap().unwrap_err();
    assert!(output.contains("must be a regular file"), "{output}");
    // The CI step refuses it too, run as the summary gives it.
    write_verifier(root.path(), None).unwrap();
    let Ok(ci) = Command::new("python3")
        .args(["-I", ".horizon/verify-pins.py"])
        .current_dir(root.path())
        .output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&ci.stderr);
    assert!(!ci.status.success());
    assert!(text.contains(".horizon/pins.json is not a regular file"), "{text}");
}

#[test]
fn an_agent_written_verifier_is_replaced_even_without_pins() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".horizon")).unwrap();
    std::fs::write(root.path().join(".horizon").join(FILE), "import os; os.system('id')\n").unwrap();
    write_verifier(root.path(), None).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join(".horizon").join(FILE)).unwrap(),
        VERIFIER
    );
}

#[test]
fn a_verifier_the_agent_changed_after_horizon_wrote_it_fails_the_check() {
    let root = repository(PINS, "ARG DOTNET_SDK_VERSION=11.0.100\n");
    write_verifier(root.path(), None).unwrap();
    std::fs::write(root.path().join(".horizon").join(FILE), "import os\n").unwrap();
    let setup = Setup::read(root.path(), None).unwrap();
    let Some(Err(error)) = check(root.path(), &setup) else {
        panic!("the check must refuse");
    };
    assert!(error.contains("is not Horizon's verifier"), "{error}");
}

#[test]
fn a_verifier_the_agent_deleted_after_horizon_wrote_it_fails_the_check() {
    let root = repository(PINS, "ARG DOTNET_SDK_VERSION=11.0.100\n");
    write_verifier(root.path(), None).unwrap();
    std::fs::remove_file(root.path().join(".horizon").join(FILE)).unwrap();
    let setup = Setup::read(root.path(), None).unwrap();
    let Some(Err(error)) = check(root.path(), &setup) else {
        panic!("the check must refuse");
    };
    assert!(error.contains("is not Horizon's verifier"), "{error}");
}

#[test]
fn a_linked_setup_folder_receives_no_verifier() {
    let root = repository(PINS, "ARG DOTNET_SDK_VERSION=11.0.100\n");
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::write(elsewhere.path().join("pins.json"), PINS).unwrap();
    std::fs::remove_dir_all(root.path().join(".horizon")).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), root.path().join(".horizon")).unwrap();
    assert!(write_verifier(root.path(), None).is_err());
    assert!(
        !elsewhere.path().join(FILE).exists(),
        "nothing is written through the link"
    );
}

#[test]
fn a_repository_without_pins_is_not_checked() {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    std::fs::create_dir(root.path().join(".horizon")).unwrap();
    write_verifier(root.path(), None).unwrap();
    assert!(!root.path().join(".horizon").join(FILE).exists());
    assert!(check(root.path(), &Setup::read(root.path(), None).unwrap()).is_none());
}

#[test]
fn a_declaration_with_a_byte_order_mark_reads_as_ci_reads_it() {
    let root = repository(
        &format!("\u{feff}{PINS}"),
        "ARG DOTNET_SDK_VERSION=11.0.100\nARG NODE_VERSION=22.1.0\nARG MONGODB_VERSION=8.0.4\n",
    );
    let output = check_output(root.path()).unwrap().unwrap();
    assert!(output.contains("DOTNET_SDK_VERSION=11.0.100"), "{output}");
}

#[test]
fn an_indented_word_ends_only_a_dash_heredoc() {
    let dockerfile = "RUN cat <<EOF\n  EOF\nARG DOTNET_SDK_VERSION=11.0.100\nEOF\nRUN cat <<-END\n\tEND\nARG OTHER=1\n";
    let root = repository(PINS, dockerfile);
    let output = check_output(root.path()).unwrap().unwrap_err();
    assert!(
        output.contains("no listed Dockerfile has ARG DOTNET_SDK_VERSION"),
        "{output}"
    );
}

#[test]
fn a_declaration_of_another_version_fails() {
    let root = repository(
        &PINS.replacen("\"version\": 1", "\"version\": 2", 1),
        "ARG DOTNET_SDK_VERSION=11.0.100\n",
    );
    let output = check_output(root.path()).unwrap().unwrap_err();
    assert!(output.contains("\"version\": 1"), "{output}");
}

#[test]
fn an_xml_source_in_the_msbuild_namespace_is_read() {
    let root = repository(
        PINS,
        "ARG DOTNET_SDK_VERSION=11.0.100\nARG NODE_VERSION=22.1.0\nARG MONGODB_VERSION=8.0.4\n",
    );
    let path = root.path();
    std::fs::write(
        path.join("Directory.Build.props"),
        r#"<Project xmlns="http://schemas.microsoft.com/developer/msbuild/2003"><PropertyGroup><MongoDBVersion>8.0.4</MongoDBVersion></PropertyGroup></Project>"#,
    )
    .unwrap();
    git(path, &["add", "Directory.Build.props"]);
    git(
        path,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "ns",
        ],
    );
    let output = check_output(path).unwrap().unwrap();
    assert!(output.contains("MONGODB_VERSION=8.0.4"), "{output}");
}

#[test]
fn a_stage_arg_without_a_default_needs_a_global_one() {
    let unpinned = "FROM base AS build\nARG DOTNET_SDK_VERSION\nFROM base\nARG DOTNET_SDK_VERSION=11.0.100\nARG NODE_VERSION=22.1.0\nARG MONGODB_VERSION=8.0.4\n";
    let output = check_output(repository(PINS, unpinned).path()).unwrap().unwrap_err();
    assert!(
        output.contains("without a default, and no global ARG gives one"),
        "{output}"
    );
    let inherited = "ARG DOTNET_SDK_VERSION=11.0.100\nFROM base\nARG DOTNET_SDK_VERSION\nARG NODE_VERSION=22.1.0\nARG MONGODB_VERSION=8.0.4\n";
    assert!(check_output(repository(PINS, inherited).path()).unwrap().is_ok());
}

#[test]
fn a_pin_source_named_like_the_verifier_elsewhere_is_read() {
    let pins = r#"{"version": 1, "pins": [{"arg": "TOOL_VERSION", "file": "tools/verify-pins.py", "regex": "VERSION = \"(\\S+)\""}]}"#;
    let root = repository(pins, "ARG TOOL_VERSION=2.0\n");
    let path = root.path();
    std::fs::create_dir_all(path.join("tools")).unwrap();
    std::fs::write(path.join("tools/verify-pins.py"), "VERSION = \"2.0\"\n").unwrap();
    git(path, &["add", "tools"]);
    git(
        path,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "tools",
        ],
    );
    let output = check_output(path).unwrap().unwrap();
    assert!(output.contains("TOOL_VERSION=2.0"), "{output}");
}

#[test]
fn a_linked_pin_source_in_the_setup_is_followed() {
    let pins =
        r#"{"version": 1, "pins": [{"arg": "TOOL_VERSION", "file": ".horizon/current", "regex": "TOOL=(\\S+)"}]}"#;
    let root = repository(pins, "ARG TOOL_VERSION=3.1\n");
    std::fs::write(root.path().join(".horizon/versions.env"), "TOOL=3.1\n").unwrap();
    std::os::unix::fs::symlink("versions.env", root.path().join(".horizon/current")).unwrap();
    let output = check_output(root.path()).unwrap().unwrap();
    assert!(output.contains("TOOL_VERSION=3.1"), "{output}");
}

#[test]
fn a_committed_link_into_the_setup_reads_the_snapshot() {
    let pins = r#"{"version": 1, "pins": [{"arg": "TOOL_VERSION", "file": "tools/current", "regex": "TOOL=(\\S+)"}]}"#;
    let root = repository(pins, "ARG TOOL_VERSION=4.0\n");
    let path = root.path();
    std::fs::write(path.join(".horizon/version"), "TOOL=3.0\n").unwrap();
    std::fs::create_dir_all(path.join("tools")).unwrap();
    std::os::unix::fs::symlink("../.horizon/version", path.join("tools/current")).unwrap();
    git(path, &["add", "tools", ".horizon/version"]);
    git(
        path,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "link",
        ],
    );
    // The proposed setup moves the version on; the old commit still says 3.0.
    std::fs::write(path.join(".horizon/version"), "TOOL=4.0\n").unwrap();
    let output = check_output(path).unwrap().unwrap();
    assert!(output.contains("TOOL_VERSION=4.0"), "{output}");
}

#[test]
fn a_regex_that_backtracks_without_end_fails_the_pin_in_time() {
    let pins = r#"{"version": 1, "pins": [
        {"arg": "TOOL_VERSION", "file": "tool.txt", "regex": "^(a+)+$"},
        {"arg": "DOTNET_SDK_VERSION", "file": "global.json", "json": "sdk.version"}]}"#;
    let root = repository(pins, "ARG TOOL_VERSION=1\nARG DOTNET_SDK_VERSION=11.0.100\n");
    std::fs::write(root.path().join("tool.txt"), format!("{}b\n", "a".repeat(64))).unwrap();
    git(root.path(), &["add", "tool.txt"]);
    git(
        root.path(),
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "tool",
        ],
    );
    let started = std::time::Instant::now();
    let output = check_output(root.path()).unwrap().unwrap_err();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "{:?}",
        started.elapsed()
    );
    assert!(output.contains("TOOL_VERSION: cannot read tool.txt"), "{output}");
    assert!(output.contains("no answer within"), "{output}");
    assert!(
        !output.contains("DOTNET_SDK_VERSION:"),
        "the other pins are still checked: {output}"
    );
}

#[test]
fn a_pin_source_in_git_metadata_is_refused_in_any_spelling() {
    for file in [".GIT/config", ".git./config", "./.Git/HEAD"] {
        let pins =
            format!(r#"{{"version": 1, "pins": [{{"arg": "TOOL_VERSION", "file": "{file}", "regex": "(.+)"}}]}}"#);
        let root = repository(&pins, "ARG TOOL_VERSION=1\n");
        let output = check_output(root.path()).unwrap().unwrap_err();
        assert!(output.contains("is inside .git"), "{file}: {output}");
    }
    assert_eq!(normal(".GIT/config"), None);
    assert_eq!(normal("tools/.git. /x"), None);
    assert_eq!(normal("tools/.github/x").as_deref(), Some("tools/.github/x"));
}

#[test]
fn the_toml_reader_without_tomllib_reads_strings_as_toml_does() {
    // Python 3.8 to 3.10 have no tomllib: the verifier's own reader runs instead.
    let script = "import json, os, sys\n\
        sys.modules['tomllib'] = None\n\
        sys.argv = ['verify-pins.py', '/nonexistent/.horizon']\n\
        namespace = {'__name__': 'verify_pins'}\n\
        exec(compile(sys.stdin.read(), 'verify-pins.py', 'exec'), namespace)\n\
        data = namespace['load_toml'](os.environ['TOML_TEXT'])\n\
        print(json.dumps(data, default=lambda value: 'unsupported'))\n";
    let toml = "[toolchain]\nchannel = \"1.85.0\" # the pinned one\nprofile = 'minimal#x'\n\
        note = \"a \\\"b\\\" \\u00e9 #\"\ncomponents = [\n  \"rustfmt\",\n]\n\
        long = \"\"\"\nchannel = \"wrong\"\n\"\"\"\n[[bin]]\nchannel = \"other\"\n";
    let Ok(mut child) = Command::new("python3")
        .args(["-I", "-c", script])
        .env("TOML_TEXT", toml)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
    else {
        return;
    };
    child.stdin.take().unwrap().write_all(VERIFIER.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let data: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let toolchain = &data["toolchain"];
    assert_eq!(
        toolchain["channel"], "1.85.0",
        "a multi-line string or array of tables changes nothing"
    );
    assert_eq!(toolchain["profile"], "minimal#x", "a # inside a string is no comment");
    assert_eq!(toolchain["note"], "a \"b\" \u{e9} #", "escapes read as TOML reads them");
    assert_eq!(toolchain["components"], "unsupported");
    assert_eq!(toolchain["long"], "unsupported");
}

#[test]
fn a_copy_of_the_setup_folder_reads_its_pins_beside_the_script() {
    // As an image does: COPY Dockerfile pins.json verify-pins.py /opt/app-horizon/, and
    // the proof or the CI runs python3 /opt/app-horizon/verify-pins.py.
    let opt = tempfile::tempdir().unwrap();
    let copy = opt.path().join("app-horizon");
    std::fs::create_dir(&copy).unwrap();
    std::fs::write(copy.join("Dockerfile"), "FROM base\nARG TOOL_VERSION=3.1\n").unwrap();
    std::fs::write(
        copy.join("pins.json"),
        r#"{"version": 1, "pins": [{"arg": "TOOL_VERSION", "file": "versions.env", "regex": "TOOL=(\\S+)"}]}"#,
    )
    .unwrap();
    std::fs::write(copy.join(FILE), VERIFIER).unwrap();
    // A pin source resolves from the folder around the copy, as before.
    std::fs::write(opt.path().join("versions.env"), "TOOL=3.1\n").unwrap();
    let Ok(output) = Command::new("python3")
        .arg("-I")
        .arg(copy.join(FILE))
        .current_dir(std::env::temp_dir())
        .output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("TOOL_VERSION=3.1"), "{text}");
    // The safety rules stay: no source outside the folder around the copy.
    std::fs::write(
        copy.join("pins.json"),
        r#"{"version": 1, "pins": [{"arg": "TOOL_VERSION", "file": "../outside", "regex": "(.+)"}]}"#,
    )
    .unwrap();
    let refused = Command::new("python3").arg("-I").arg(copy.join(FILE)).output().unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("is outside the repository"));
}

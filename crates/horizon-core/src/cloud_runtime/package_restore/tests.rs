use super::*;
use crate::cloud_runtime::Cancellation;
use std::process::Command;

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid"])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A repository whose committed `restore.sh` restores into its first argument, and a
/// file that exists only in the local checkout.
fn repository(root: &Path, script: &str) -> (PathBuf, String) {
    let path = root.join("app");
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "--quiet"]);
    std::fs::write(path.join("app.csproj"), "<Project />\n").unwrap();
    std::fs::write(path.join("restore.sh"), script).unwrap();
    git(&path, &["add", "."]);
    git(&path, &["update-index", "--chmod=+x", "restore.sh"]);
    git(&path, &["commit", "--quiet", "-m", "Add fixture"]);
    std::fs::write(path.join("local-only"), "uncommitted\n").unwrap();
    let revision = git(&path, &["rev-parse", "HEAD"]);
    (path, revision)
}

#[cfg(unix)]
const RESTORE: &str = "set -e\n\
test -f app.csproj\n\
test ! -e local-only\n\
mkdir -p \"$1/example.package/1.0.0/lib\"\n\
printf 'library' > \"$1/example.package/1.0.0/lib/example.dll\"\n\
printf '#!/bin/sh\\n' > \"$1/example.package/1.0.0/tool\"\n\
chmod +x \"$1/example.package/1.0.0/tool\"\n";

fn packages(restore: &[&str]) -> Packages {
    Packages {
        restore: restore.iter().map(|argument| (*argument).to_owned()).collect(),
        env: "NUGET_PACKAGES".into(),
    }
}

fn run(
    repository: &Path,
    revision: &str,
    packages: &Packages,
    approvals: &[Approval],
    root: &Path,
) -> Result<Restored> {
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    restore(repository, revision, packages, approvals, root, &runner)
}

#[test]
#[cfg(unix)]
fn only_an_approved_restore_runs_and_it_sees_only_the_committed_tree() {
    let temp = tempfile::tempdir().unwrap();
    let (app, revision) = repository(temp.path(), RESTORE);
    let packages = packages(&["sh", "restore.sh", "{dir}"]);
    let unapproved = temp.path().join("unapproved");
    std::fs::create_dir(&unapproved).unwrap();
    for approvals in [
        vec![],
        vec![Approval::new(temp.path().join("other"), &packages)],
        vec![Approval::new(
            app.clone(),
            &self::packages(&["sh", "other.sh", "{dir}"]),
        )],
        vec![Approval {
            env: "OTHER".into(),
            ..Approval::new(app.clone(), &packages)
        }],
    ] {
        assert!(matches!(
            run(&app, &revision, &packages, &approvals, &unapproved),
            Err(Error::PackageRestoreNotApproved)
        ));
    }
    assert_eq!(
        std::fs::read_dir(&unapproved).unwrap().count(),
        0,
        "nothing ran for an unapproved restore"
    );

    let root = temp.path().join("approved");
    std::fs::create_dir(&root).unwrap();
    let approval = Approval::new(app.join("."), &packages);
    let restored = run(&app, &revision, &packages, &[approval], &root).unwrap();
    let names: Vec<_> = restored.entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "example.package",
            "example.package/1.0.0",
            "example.package/1.0.0/lib",
            "example.package/1.0.0/lib/example.dll",
            "example.package/1.0.0/tool",
        ]
    );
    assert_eq!(
        (restored.files(), restored.bytes, restored.env.as_str()),
        (2, 17, "NUGET_PACKAGES")
    );
    let tool = restored.entries.last().unwrap();
    let library = &restored.entries[3];
    assert!(tool.executable && !library.executable);
    assert!(
        !root.join("package-restore-tree").exists(),
        "the exported tree is removed once the restore ends"
    );
}

#[test]
#[cfg(unix)]
fn a_committed_program_path_runs_the_committed_program() {
    let temp = tempfile::tempdir().unwrap();
    let (app, revision) = repository(temp.path(), &format!("#!/bin/sh\n{RESTORE}"));
    let packages = packages(&["./restore.sh", "{dir}"]);
    let root = temp.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let restored = run(
        &app,
        &revision,
        &packages,
        &[Approval::new(app.clone(), &packages)],
        &root,
    )
    .unwrap();
    assert_eq!(restored.files(), 2);
}

#[test]
#[cfg(unix)]
fn a_failed_restore_and_a_folder_with_credentials_or_links_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    for (case, script) in [
        ("failed", "exit 3\n"),
        ("settings", "printf '<configuration />' > \"$1/NuGet.Config\"\n"),
        (
            "nested settings",
            "mkdir \"$1/a\" && printf 'token' > \"$1/a/.npmrc\"\n",
        ),
        ("link", "ln -s /etc/hostname \"$1/hostname\"\n"),
        (
            "docker",
            "mkdir \"$1/.docker\" && printf '{}' > \"$1/.docker/config.json\"\n",
        ),
    ] {
        let case_root = temp.path().join(case);
        let (app, revision) = repository(&case_root, script);
        let packages = packages(&["sh", "restore.sh", "{dir}"]);
        let root = case_root.join("root");
        std::fs::create_dir(&root).unwrap();
        let result = run(
            &app,
            &revision,
            &packages,
            &[Approval::new(app.clone(), &packages)],
            &root,
        );
        match case {
            "failed" => assert!(matches!(result, Err(Error::Command("Package restore"))), "{case}"),
            "link" => assert!(
                matches!(&result, Err(Error::Invalid(message)) if message.contains("link")),
                "{case}"
            ),
            _ => assert!(
                matches!(&result, Err(Error::Invalid(message)) if message.contains("settings file")),
                "{case}"
            ),
        }
    }
}

#[test]
#[cfg(unix)]
fn the_archive_carries_the_folder_and_the_manifest_names_it() {
    let temp = tempfile::tempdir().unwrap();
    let (app, revision) = repository(temp.path(), RESTORE);
    let packages = packages(&["sh", "restore.sh", "{dir}"]);
    let root = temp.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let restored = run(
        &app,
        &revision,
        &packages,
        &[Approval::new(app.clone(), &packages)],
        &root,
    )
    .unwrap();
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let archive = crate::cloud_runtime::repository::auxiliary_with_packages(
        &app,
        &revision,
        &root,
        &horizon_cloud::Source::default(),
        Some(&restored),
        &runner,
    )
    .unwrap();
    let extracted = temp.path().join("extracted");
    std::fs::create_dir(&extracted).unwrap();
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&extracted)
        .status()
        .unwrap();
    assert!(status.success());
    let library = extracted.join("packages/example.package/1.0.0/lib/example.dll");
    assert_eq!(std::fs::read_to_string(library).unwrap(), "library");
    assert!(executable(
        &std::fs::metadata(extracted.join("packages/example.package/1.0.0/tool")).unwrap()
    ));
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(extracted.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["packages"],
        serde_json::json!({"env": "NUGET_PACKAGES", "files": 2, "bytes": 17})
    );
}

#[test]
fn approvals_need_an_absolute_checkout_path() {
    let packages = packages(&["sh", "restore.sh", "{dir}"]);
    let absolute = tempfile::tempdir().unwrap();
    assert!(Approval::new(PathBuf::from("app"), &packages).validate().is_err());
    assert!(Approval::new(absolute.path().join("app"), &packages).validate().is_ok());
}

#[test]
fn a_restore_runs_on_every_platform() {
    // Git is on every host Horizon deploys from, so it stands in for a package manager.
    let temp = tempfile::tempdir().unwrap();
    let (app, revision) = repository(temp.path(), "unused\n");
    let packages = packages(&["git", "-C", "{dir}", "init", "--quiet"]);
    let root = temp.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let restored = run(
        &app,
        &revision,
        &packages,
        &[Approval::new(app.clone(), &packages)],
        &root,
    )
    .unwrap();
    assert!(restored.entries.iter().any(|entry| entry.name == ".git/HEAD"));
    assert!(restored.files() > 0 && restored.bytes > 0);
}

#[test]
fn credential_paths_count_only_in_their_managers_folder() {
    assert!(credential("", "NuGet.Config") && credential("deep", ".npmrc"));
    assert!(credential(".docker", "config.json") && credential(".M2", "settings.xml"));
    assert!(!credential("example.package", "config.json") && !credential("lib", "settings.xml"));
}

#[test]
fn a_cancelled_deployment_stops_the_scan() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("folder")).unwrap();
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(scan(temp.path(), &cancel).is_err());
}

#[test]
#[cfg(unix)]
fn a_relative_program_never_leaves_the_exported_tree() {
    let temp = tempfile::tempdir().unwrap();
    let tree = temp.path().join("tree");
    std::fs::create_dir_all(tree.join("tools")).unwrap();
    std::fs::write(tree.join("tools/restore.sh"), "").unwrap();
    std::fs::write(temp.path().join("outside.sh"), "").unwrap();
    std::os::unix::fs::symlink(temp.path().join("outside.sh"), tree.join("tools/linked.sh")).unwrap();
    assert_eq!(program(&tree, "dotnet").unwrap(), PathBuf::from("dotnet"));
    assert_eq!(
        program(&tree, "/usr/bin/dotnet").unwrap(),
        PathBuf::from("/usr/bin/dotnet")
    );
    assert_eq!(
        program(&tree, "./tools/restore.sh").unwrap(),
        tree.canonicalize().unwrap().join("tools/restore.sh")
    );
    for escaping in [
        "../outside.sh",
        "tools/../../outside.sh",
        "./tools/linked.sh",
        "./tools/missing.sh",
        "./tools",
    ] {
        assert!(program(&tree, escaping).is_err(), "{escaping}");
    }
}

#[test]
fn a_folder_over_the_entry_limit_is_refused_while_it_is_listed() {
    let temp = tempfile::tempdir().unwrap();
    for index in 0..5 {
        std::fs::write(temp.path().join(format!("file-{index}")), "x").unwrap();
    }
    let cancel = Cancellation::default();
    assert_eq!(scan_within(temp.path(), &cancel, 5).unwrap().0.len(), 5);
    assert!(matches!(
        scan_within(temp.path(), &cancel, 4),
        Err(Error::Invalid(message)) if message.contains("500,000")
    ));
}

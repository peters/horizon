use super::*;
use crate::cloud_runtime::project_reservations::{journal::Journal, source};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path, process::Command};

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
fn init(path: &Path) {
    fs::create_dir(path).unwrap();
    git(path, &["init", "-q"]);
    git(path, &["config", "user.name", "Fixture"]);
    git(path, &["config", "user.email", "fixture@example.invalid"]);
}
fn repository(path: &Path, name: &str) -> String {
    init(path);
    fs::write(path.join("value"), format!("history {name}")).unwrap();
    git(path, &["add", "value"]);
    git(path, &["commit", "-qm", "Initial source"]);
    let module = path.join("module");
    init(&module);
    fs::write(module.join("value"), format!("module {name}")).unwrap();
    git(&module, &["add", "value"]);
    git(&module, &["commit", "-qm", "Module source"]);
    let pinned = git(&module, &["rev-parse", "HEAD"]);
    let content = format!("large asset {name}");
    let mut oid = String::new();
    for byte in Sha256::digest(content.as_bytes()) {
        use std::fmt::Write;
        write!(oid, "{byte:02x}").unwrap();
    }
    let media = path.join(".git/lfs/objects").join(&oid[..2]).join(&oid[2..4]);
    fs::create_dir_all(&media).unwrap();
    fs::write(media.join(&oid), &content).unwrap();
    fs::write(
        path.join("asset"),
        format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:{oid}\nsize {}\n",
            content.len()
        ),
    )
    .unwrap();
    fs::write(path.join(".gitattributes"), "asset filter=lfs\n").unwrap();
    fs::write(path.join("value"), format!("committed {name}")).unwrap();
    git(path, &["add", "value", "asset", ".gitattributes"]);
    git(
        path,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{pinned},module"),
        ],
    );
    git(path, &["commit", "-qm", "Pin source material"]);
    fs::write(path.join("value"), "uncommitted sentinel").unwrap();
    fs::write(module.join("value"), "uncommitted module sentinel").unwrap();
    git(path, &["rev-parse", "HEAD"])
}
fn prepare_host(f: &mut CoordinatorFixture, request: &Reservation, worker: &mut Manifest) {
    for change in [
        Change::Reserve(request.clone()),
        Change::PrepareNamespace(request.project.clone()),
    ] {
        transact(f, &change, &mut |_, _, bytes| Ok(apply(worker, bytes))).unwrap();
    }
}

#[test]
fn retained_source_artifacts_survive_reopen_and_later_local_changes() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repository");
    let selected = repository(&repo, "one");
    let mut f = ready();
    let request = reservation(&f, "one", 8000);
    let mut worker = remote(&f);
    prepare_host(&mut f, &request, &mut worker);
    let cancellation = Cancellation::default();
    let descriptor = source::prepare(&mut f.owner, &request.project, &repo, "HEAD", &cancellation).unwrap();
    assert_eq!(descriptor.revision, selected);
    let before = Journal::load(&f.owner).unwrap().unwrap();
    let root = f.owner.artifact_root().unwrap().to_owned();
    let mut saved = Vec::new();
    let mut frame = Vec::new();
    assert!(
        transact(
            &mut f,
            &Change::ImportSource(request.project.clone(), descriptor.clone()),
            &mut |_, command, bytes| {
                saved = bytes.to_vec();
                if command.ends_with("import-project-source") {
                    source::for_request(&before, bytes)?
                        .frame(&root, bytes, &cancellation)?
                        .read_to_end(&mut frame)?;
                    return Err(ReservationError::Invalid);
                }
                Ok(apply(&mut worker, bytes))
            }
        )
        .is_err()
    );
    let pending = Journal::load(&f.owner).unwrap().unwrap();
    let source_limit = crate::cloud_runtime::project_reservations::timeout_limit(&Change::Resume, Some(&pending));
    assert_eq!(
        source_limit,
        horizon_cloud_protocol::membership::Source::CONTROLLER_TIMEOUT
    );
    assert!(source_limit > horizon_cloud_protocol::membership::Source::WORKER_TIMEOUT * 2);
    assert_eq!(
        crate::cloud_runtime::project_reservations::timeout_limit(
            &Change::Cancel(request.project.clone()),
            Some(&pending)
        ),
        std::time::Duration::from_secs(180)
    );
    assert!(
        transact(&mut f, &Change::Cancel(request.project.clone()), &mut |_, _, _| panic!(
            "pending import"
        ))
        .is_err()
    );
    git(&repo, &["add", "value"]);
    git(&repo, &["commit", "-qm", "Later local source"]);
    f = f.reopen();
    assert_eq!(
        source::prepare(&mut f.owner, &request.project, &repo, "HEAD", &cancellation).unwrap(),
        descriptor
    );
    let reopened = Journal::load(&f.owner).unwrap().unwrap();
    transact(&mut f, &Change::Resume, &mut |_, command, bytes| {
        assert_eq!(bytes, saved);
        if command.ends_with("import-project-source") {
            let mut repeated = Vec::new();
            source::for_request(&reopened, bytes)?
                .frame(&root, bytes, &cancellation)?
                .read_to_end(&mut repeated)?;
            assert_eq!(repeated, frame);
        }
        Ok(apply(&mut worker, bytes))
    })
    .unwrap();
    assert_eq!(worker.members[0].state, State::Importing);
    assert!(source::prepare(&mut f.owner, &request.project, &repo, "HEAD~1", &cancellation).is_err());
    let payload = journal(&f.owner);
    let directory = payload["sources"][0]["directory"].as_str().unwrap();
    let retained = fs::read_dir(root.join(directory))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(retained, ["pack".into(), "source-material.tar".into()].into());
    fs::remove_file(root.join(directory).join("pack")).unwrap();
    fs::write(root.join(directory).join("pack"), b"replacement").unwrap();
    assert!(source::prepare(&mut f.owner, &request.project, &repo, "HEAD", &cancellation).is_err());
}

#[test]
fn source_generation_never_writes_beyond_the_shared_pack_material_and_archive_budget() {
    fn size(path: &Path) -> u64 {
        fs::read_dir(path)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let meta = entry.metadata().unwrap();
                if meta.is_dir() { size(&entry.path()) } else { meta.len() }
            })
            .sum()
    }
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repository");
    let revision = repository(&repo, "budget");
    let retained = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let cancellation = Cancellation::default();
    let runner = runner(&cancellation);
    let export = |retained: &Path, scratch: &Path, limit| {
        crate::cloud_runtime::repository::bounded_source(&repo, &revision, retained, scratch, &runner, limit)
    };
    export(retained.path(), scratch.path(), 1024 * 1024).unwrap();
    let pack = fs::metadata(retained.path().join("pack")).unwrap().len();
    let header = horizon_cloud_protocol::membership::Source::MAX_REQUEST_BYTES as u64 + 4;
    let total = header + 2 * size(retained.path()) + size(scratch.path());
    for limit in [header + 2 * pack - 1, header + 2 * pack + 1, total - 1, total] {
        let retained = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        assert_eq!(export(retained.path(), scratch.path(), limit).is_ok(), limit == total);
        assert!(header + 2 * size(retained.path()) + size(scratch.path()) <= limit);
    }
}

#[test]
fn source_frame_budget_includes_retained_files_and_rejects_growth() {
    use std::{io::Write, os::unix::fs::PermissionsExt};
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repository");
    repository(&repo, "frame");
    let mut f = ready();
    let reservation = reservation(&f, "one", 8000);
    let mut worker = remote(&f);
    prepare_host(&mut f, &reservation, &mut worker);
    let cancellation = Cancellation::default();
    let descriptor = source::prepare(&mut f.owner, &reservation.project, &repo, "HEAD", &cancellation).unwrap();
    let saved = Journal::load(&f.owner).unwrap().unwrap();
    let artifacts = &saved.sources[0];
    let root = f.owner.artifact_root().unwrap();
    let request = b"bounded frame fixture";
    let retained = descriptor.pack.length + descriptor.material.length;
    let limit = 2 * retained + request.len() as u64 + 4;
    assert!(
        artifacts
            .frame_with_limit(root, request, &cancellation, limit - 1)
            .is_err()
    );
    let frame = artifacts.frame_with_limit(root, request, &cancellation, limit).unwrap();
    assert_eq!(retained + frame.metadata().unwrap().len(), limit);
    drop(frame);
    let payload = journal(&f.owner);
    let directory = payload["sources"][0]["directory"].as_str().unwrap();
    let pack = root.join(directory).join("pack");
    fs::set_permissions(&pack, fs::Permissions::from_mode(0o600)).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(pack)
        .unwrap()
        .write_all(b"x")
        .unwrap();
    assert!(artifacts.frame_with_limit(root, request, &cancellation, limit).is_err());
}

#[test]
fn source_generation_rejects_asset_fifo_substitution_without_waiting_for_a_writer() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repository");
    let revision = repository(&repo, "fifo");
    let pointer = fs::read_to_string(repo.join("asset")).unwrap();
    let oid = pointer.lines().nth(1).unwrap().strip_prefix("oid sha256:").unwrap();
    let asset = repo.join(".git/lfs/objects").join(&oid[..2]).join(&oid[2..4]).join(oid);
    let replaced = std::cell::Cell::new(false);
    let cancellation = Cancellation::default();
    let emit = |event| {
        if let crate::cloud_runtime::Event::Progress(progress) = event
            && progress.detail == "Bounded source export"
            && !replaced.replace(true)
        {
            fs::rename(&asset, asset.with_extension("saved")).unwrap();
            assert!(Command::new("mkfifo").arg(&asset).status().unwrap().success());
        }
    };
    let runner = crate::cloud_runtime::command::Runner {
        cancel: &cancellation,
        emit: &emit,
        secrets: Vec::new(),
    };
    let retained = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    assert!(
        crate::cloud_runtime::repository::bounded_source(
            &repo,
            &revision,
            retained.path(),
            scratch.path(),
            &runner,
            1024 * 1024,
        )
        .is_err()
    );
    assert!(replaced.get());
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(fs::read(asset.with_extension("saved")).unwrap(), b"large asset fifo");
}

#[test]
fn artifact_creation_rejects_replaced_directory_and_owner_without_touching_replacements() {
    use std::os::unix::fs::PermissionsExt;
    for parent in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repository");
        repository(&repo, "source");
        let mut f = ready();
        let request = reservation(&f, "one", 8000);
        let mut worker = remote(&f);
        prepare_host(&mut f, &request, &mut worker);
        let mut replacement = PathBuf::new();
        assert!(
            source::prepare_with(
                &mut f.owner,
                &request.project,
                &repo,
                "HEAD",
                &Cancellation::default(),
                &mut |path| {
                    replacement = if parent {
                        path.parent().unwrap().into()
                    } else {
                        path.into()
                    };
                    fs::rename(&replacement, replacement.with_extension("original"))?;
                    fs::create_dir(&replacement)?;
                    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o700))?;
                    fs::write(replacement.join("sentinel"), b"preserve")?;
                    Ok(())
                }
            )
            .is_err()
        );
        assert_eq!(fs::read(replacement.join("sentinel")).unwrap(), b"preserve");
        assert_eq!(fs::read_dir(replacement).unwrap().count(), 1);
    }
}

#[test]
#[ignore = "requires scripts/cloud-initialization-smoke.py --scenario sources and an isolated mount namespace"]
fn native_ssh_project_sources() {
    let directory = PathBuf::from(std::env::var_os("HORIZON_INITIALIZATION_FIXTURE").unwrap());
    let port = u16::try_from(serde_json::from_slice::<serde_json::Value>(&fs::read(directory.join("fixture.json")).unwrap()).unwrap()["port"].as_u64().unwrap()).unwrap();
    let mut f = native_fixture(&directory);
    let cancellation = Cancellation::default();
    let runner = runner(&cancellation);
    let target = target(&f.record, ([127, 0, 0, 1], port).into()).unwrap();
    enroll(&f.record, &target, &runner, startup_deadline()).unwrap();
    initialize(&mut f.owner, &mut f.record, &target, &runner, startup_deadline()).unwrap();
    complete(&mut f.owner, &mut f.record, &target, &cancellation, startup_deadline()).unwrap();
    let mut projects = Vec::new();
    for (index, name) in ["one", "two", "three"].iter().enumerate() {
        let request = reservation(&f, name, 8000 + u16::try_from(index).unwrap());
        f = native_prepare(f, &target, &runner, &directory, &request, 1);
        let repo = directory.join(format!("repo-{name}"));
        let revision = repository(&repo, name);
        let descriptor = source::prepare(&mut f.owner, &request.project, &repo, "HEAD", &cancellation).unwrap();
        let saved = Journal::load(&f.owner).unwrap().unwrap();
        let root = f.owner.artifact_root().unwrap().to_owned();
        let exchange = |connection: &Connection, command: &str, bytes: &[u8]| -> reservations::Result<Vec<u8>> {
            let mut command_line = connection.pinned_command(command);
            Ok(if command.ends_with("import-project-source") {
                runner.private_file_exchange(
                    &mut command_line,
                    source::for_request(&saved, bytes)?.frame(&root, bytes, &cancellation)?,
                    Duration::from_secs(120),
                )?
            } else {
                runner.private_exchange(&mut command_line, bytes, Duration::from_secs(90))?
            })
        };
        let vault = f.vault.clone();
        let result = reservations::coordinate(
            &mut f.owner,
            &target,
            &request.image_digest,
            &Change::ImportSource(request.project.clone(), descriptor),
            &mut |connection, command, bytes| {
                let reply = exchange(connection, command, bytes)?;
                if command.ends_with("import-project-source") {
                    if index == 0 {
                        return Err(ReservationError::Invalid);
                    }
                    if index == 2 {
                        vault.fail_after(Some(0));
                    }
                }
                Ok(reply)
            },
        );
        if index == 1 {
            result.unwrap();
        } else {
            assert!(result.is_err());
            fs::write(directory.join("restart"), "restart source recovery").unwrap();
            f = f.reopen();
            reservations::coordinate(
                &mut f.owner,
                &target,
                &request.image_digest,
                &Change::Resume,
                &mut |connection, command, bytes| exchange(connection, command, bytes),
            )
            .unwrap();
        }
        let source_root = directory
            .join("workspace/projects")
            .join(request.project.project_id().to_string())
            .join("repository/source");
        verify_import(&source_root, &revision, name);
        projects.push((request, source_root));
    }
    let before: Vec<_> = projects
        .iter()
        .map(|(_, root)| fs::read(root.join("repository.git/HEAD")).unwrap())
        .collect();
    reservations::coordinate(
        &mut f.owner,
        &target,
        &projects[0].0.image_digest,
        &Change::Cancel(projects[0].0.project.clone()),
        &mut |connection, command, bytes| {
            Ok(runner.private_exchange(&mut connection.pinned_command(command), bytes, Duration::from_secs(90))?)
        },
    )
    .unwrap();
    for ((_, root), expected) in projects.iter().zip(before) {
        assert_eq!(fs::read(root.join("repository.git/HEAD")).unwrap(), expected);
    }
    assert_bootstrap_fenced(&mut f);
}

fn verify_import(source_root: &Path, revision: &str, name: &str) {
    assert_eq!(
        git(&source_root.join("repository.git"), &["rev-parse", "HEAD"]),
        revision
    );
    assert_eq!(
        git(&source_root.join("repository.git"), &["show", "HEAD:value"]),
        format!("committed {name}")
    );
    assert_eq!(
        git(&source_root.join("repository.git"), &["show", "HEAD~1:value"]),
        format!("history {name}")
    );
    assert_eq!(
        git(&source_root.join("module-0.git"), &["show", "HEAD:value"]),
        format!("module {name}")
    );
}

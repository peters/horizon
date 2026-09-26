mod sessions;
use super::*;
use crate::cloud_runtime::project_reservations::{journal::Journal, source};
use horizon_cloud::Agent;
use horizon_cloud_protocol::membership::{Artifact, Session, Source};
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

fn declared_source() -> Source {
    Source {
        version: 1,
        revision: "a".repeat(40),
        pack: Artifact {
            length: 32,
            sha256: [1; 32],
        },
        material: Artifact {
            length: 1024,
            sha256: [2; 32],
        },
    }
}

#[test]
fn agent_session_journal_distinguishes_sessions_and_resumes_exact_signed_requests() {
    let mut f = ready();
    let mut request = reservation(&f, "one", 8000);
    request.capabilities.agents = [Agent::Codex, Agent::Claude].into();
    let mut worker = remote(&f);
    prepare_host(&mut f, &request, &mut worker);
    let descriptor = declared_source();
    transact(
        &mut f,
        &Change::ImportSource(request.project.clone(), descriptor.clone()),
        &mut |_, _, bytes| Ok(apply(&mut worker, bytes)),
    )
    .unwrap();
    let mut originals = Vec::new();
    for (index, agent) in [Agent::Codex, Agent::Claude].into_iter().enumerate() {
        let session = Session::new(agent, descriptor.revision.clone());
        let change = Change::ReserveSession(request.project.clone(), session.clone());
        let mut sent = Vec::new();
        let vault = f.vault.clone();
        assert!(
            transact(&mut f, &change, &mut |_, command, bytes| {
                assert_eq!(command, "horizon-cloud-worker reserve-project-session");
                sent = bytes.to_vec();
                let reply = apply(&mut worker, bytes);
                if index == 0 {
                    Err(ReservationError::Invalid)
                } else {
                    vault.fail_after(Some(0));
                    Ok(reply)
                }
            })
            .is_err()
        );
        let mut changed = session.clone();
        changed.revision = "b".repeat(40);
        assert!(
            transact(
                &mut f,
                &Change::ReserveSession(request.project.clone(), changed),
                &mut |_, _, _| panic!("changed pending session")
            )
            .is_err()
        );
        assert!(
            transact(&mut f, &Change::Cancel(request.project.clone()), &mut |_, _, _| panic!(
                "pending session cancellation"
            ))
            .is_err()
        );
        f = f.reopen();
        let journal = Journal::load(&f.owner).unwrap().unwrap();
        assert_eq!(
            reservations::timeout_limit(&Change::Resume, Some(&journal)),
            Source::CONTROLLER_TIMEOUT
        );
        let receipt = transact(&mut f, &Change::Resume, &mut |_, command, bytes| {
            assert_eq!(command, "horizon-cloud-worker reserve-project-session");
            assert_eq!(bytes, sent);
            Ok(apply(&mut worker, bytes))
        })
        .unwrap();
        assert_eq!(worker.members[0].sessions.len(), index + 1);
        originals.push((change, sent, receipt));
    }
    for (change, sent, receipt) in &originals {
        assert_eq!(
            transact(&mut f, change, &mut |_, _, bytes| {
                assert_eq!(bytes, sent);
                Ok(apply(&mut worker, bytes))
            })
            .unwrap(),
            *receipt
        );
    }
    let mut changed = worker.members[0].sessions[0].clone();
    changed.agent = Agent::Claude;
    assert!(
        transact(
            &mut f,
            &Change::ReserveSession(request.project.clone(), changed),
            &mut |_, _, _| panic!("changed completed session")
        )
        .is_err()
    );
    let before = worker.members[0].sessions.clone();
    transact(&mut f, &Change::Cancel(request.project), &mut |_, _, bytes| {
        Ok(apply(&mut worker, bytes))
    })
    .unwrap();
    assert_eq!(worker.members[0].sessions, before);
    assert!(transact(&mut f, &originals[0].0, &mut |_, _, _| panic!("cancelled session")).is_err());
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
                        .frame(&root, bytes, &cancellation, startup_deadline())?
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
                .frame(&root, bytes, &cancellation, startup_deadline())?
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
    let expired = std::time::Instant::now();
    assert!(matches!(
        artifacts.frame(root, request, &cancellation, expired),
        Err(ReservationError::Deadline)
    ));
    assert!(matches!(
        artifacts.verify(root, &cancellation, Some(expired)),
        Err(ReservationError::Deadline)
    ));
    let retained = descriptor.pack.length + descriptor.material.length;
    let limit = 2 * retained + request.len() as u64 + 4;
    assert!(
        artifacts
            .frame_with_limit(root, request, &cancellation, limit - 1, startup_deadline())
            .is_err()
    );
    let frame = artifacts
        .frame_with_limit(root, request, &cancellation, limit, startup_deadline())
        .unwrap();
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
    assert!(
        artifacts
            .frame_with_limit(root, request, &cancellation, limit, startup_deadline())
            .is_err()
    );
}

#[test]
fn source_collection_rejects_existing_fifo_and_accepts_regular_storage_symlinks() {
    use std::os::unix::process::CommandExt;
    let Some(root) = std::env::var_os("HORIZON_SOURCE_FIFO_FIXTURE") else {
        let temp = tempfile::tempdir().unwrap();
        let name = concat!(
            module_path!(),
            "::source_collection_rejects_existing_fifo_and_accepts_regular_storage_symlinks"
        );
        let name = name.split_once("::").unwrap().1;
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .process_group(0)
            .env("HORIZON_SOURCE_FIFO_FIXTURE", temp.path())
            .stdin(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                assert!(temp.path().join("passed").is_file(), "child must execute the test");
                return;
            }
            if std::time::Instant::now() >= deadline {
                let id = rustix::process::Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
                rustix::process::kill_process_group(id, rustix::process::Signal::KILL).unwrap();
                child.wait().unwrap();
                panic!("source collection blocked on a FIFO");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let repo = PathBuf::from(root).join("repository");
    let revision = repository(&repo, "initial-fifo");
    let pointer = fs::read_to_string(repo.join("asset")).unwrap();
    let oid = pointer.lines().nth(1).unwrap().strip_prefix("oid sha256:").unwrap();
    let asset = repo.join(".git/lfs/objects").join(&oid[..2]).join(&oid[2..4]).join(oid);
    let original = asset.with_extension("saved");
    fs::rename(&asset, &original).unwrap();
    assert!(Command::new("mkfifo").arg(&asset).status().unwrap().success());
    let cancellation = Cancellation::default();
    let runner = runner(&cancellation);
    let retained = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let result = crate::cloud_runtime::repository::bounded_source(
        &repo,
        &revision,
        retained.path(),
        scratch.path(),
        &runner,
        1024 * 1024,
    );
    assert!(matches!(
        result,
        Err(crate::cloud_runtime::Error::Invalid(
            "Local Git LFS object size mismatch"
        ))
    ));
    assert_eq!(fs::read_dir(retained.path()).unwrap().count(), 0);
    fs::remove_file(&asset).unwrap();
    std::os::unix::fs::symlink(&original, &asset).unwrap();
    crate::cloud_runtime::repository::bounded_source(
        &repo,
        &revision,
        retained.path(),
        scratch.path(),
        &runner,
        1024 * 1024,
    )
    .unwrap();
    fs::write(repo.parent().unwrap().join("passed"), b"verified").unwrap();
}

fn add_index_entries(repo: &Path, prefix: &str, count: usize, blob: &str) {
    use std::io::{Seek, Write};
    let mut input = tempfile::tempfile().unwrap();
    for index in 0..count {
        writeln!(input, "100644 {blob}\t{prefix}-{index:05}").unwrap();
    }
    input.rewind().unwrap();
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["update-index", "--index-info"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(input)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    git(repo, &["commit", "-qm", "Collection boundary fixture"]);
}

#[test]
fn source_collection_limits_tree_entries_assets_and_bytes_before_excess_hashing() {
    for (kind, expected_verifications, limit) in
        [("bytes", 0, 1), ("assets", 8192, 1024 * 1024), ("tree", 0, 1024 * 1024)]
    {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repository");
        repository(&repo, "collection");
        if kind == "assets" {
            fs::write(repo.join(".gitattributes"), "asset* filter=lfs\n").unwrap();
            git(&repo, &["add", ".gitattributes"]);
            let blob = git(&repo, &["hash-object", "-w", "asset"]);
            add_index_entries(&repo, "asset", 8192, &blob);
        } else if kind == "tree" {
            let blob = git(&repo, &["hash-object", "-w", "value"]);
            add_index_entries(&repo, "entry", 65536, &blob);
        }
        let revision = git(&repo, &["rev-parse", "HEAD"]);
        let verifications = std::cell::Cell::new(0);
        let cancellation = Cancellation::default();
        let emit = |event| {
            if let crate::cloud_runtime::Event::Progress(progress) = event
                && progress.detail == "Verifying current source asset"
                && progress.completed == 0
            {
                verifications.set(verifications.get() + 1);
            }
        };
        let runner = crate::cloud_runtime::command::Runner {
            cancel: &cancellation,
            emit: &emit,
            secrets: Vec::new(),
        };
        let retained = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let result = crate::cloud_runtime::repository::bounded_source(
            &repo,
            &revision,
            retained.path(),
            scratch.path(),
            &runner,
            limit,
        );
        let expected = match kind {
            "bytes" => "Source material exceeds its verification byte limit",
            "assets" => "Source material exceeds its asset limit",
            _ => "Source tree exceeds its entry limit",
        };
        assert!(
            matches!(result, Err(crate::cloud_runtime::Error::Invalid(message)) if message == expected),
            "{kind}"
        );
        assert_eq!(verifications.get(), expected_verifications, "{kind}");
        assert_eq!(fs::read_dir(retained.path()).unwrap().count(), 0);
    }
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
fn incomplete_generation_fences_repeated_exports_after_reopen_and_preserves_replacements() {
    for completion_save in [None, Some(0), Some(1)] {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repository");
        repository(&repo, "generation");
        let mut f = ready();
        let request = reservation(&f, "one", 8000);
        let mut worker = remote(&f);
        prepare_host(&mut f, &request, &mut worker);
        let vault = f.vault.clone();
        let mut directory = PathBuf::new();
        assert!(
            source::prepare_with(
                &mut f.owner,
                &request.project,
                &repo,
                "HEAD",
                &Cancellation::default(),
                &mut |path| {
                    directory = path.into();
                    if let Some(writes) = completion_save {
                        vault.fail_after(Some(writes));
                        Ok(())
                    } else {
                        Err(ReservationError::Invalid)
                    }
                }
            )
            .is_err()
        );
        f = f.reopen();
        let saved = Journal::load(&f.owner).unwrap().unwrap();
        let root = f.owner.artifact_root().unwrap().to_owned();
        let before = fs::read_dir(&root).unwrap().count();
        if completion_save == Some(1) {
            assert!(saved.generation.is_none());
            let pack = fs::read(directory.join("pack")).unwrap();
            let material = fs::read(directory.join("source-material.tar")).unwrap();
            assert_eq!(
                source::prepare(&mut f.owner, &request.project, &repo, "HEAD", &Cancellation::default()).unwrap(),
                saved.sources[0].descriptor
            );
            assert_eq!(fs::read_dir(&root).unwrap().count(), before);
            assert_eq!(fs::read(directory.join("pack")).unwrap(), pack);
            assert_eq!(fs::read(directory.join("source-material.tar")).unwrap(), material);
            continue;
        }
        assert!(saved.generation.is_some());
        let original = directory.with_extension("original");
        fs::rename(&directory, &original).unwrap();
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("sentinel"), b"preserve replacement").unwrap();
        for _ in 0..3 {
            assert!(matches!(
                source::prepare(&mut f.owner, &request.project, &repo, "HEAD", &Cancellation::default()),
                Err(ReservationError::Pending)
            ));
        }
        assert_eq!(fs::read_dir(&root).unwrap().count(), before + 1);
        assert_eq!(fs::read(directory.join("sentinel")).unwrap(), b"preserve replacement");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        assert_eq!(
            fs::read_dir(original).unwrap().count(),
            if completion_save.is_some() { 2 } else { 0 }
        );
    }
}

#[test]
fn unanchored_generation_never_creates_an_artifact_directory() {
    for writes in [0, 1] {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repository");
        repository(&repo, "intent");
        let mut f = ready();
        let request = reservation(&f, "one", 8000);
        let mut worker = remote(&f);
        prepare_host(&mut f, &request, &mut worker);
        let root = f.owner.artifact_root().unwrap().to_owned();
        let before = fs::read_dir(&root).unwrap().count();
        f.vault.fail_after(Some(writes));
        assert!(
            source::prepare_with(
                &mut f.owner,
                &request.project,
                &repo,
                "HEAD",
                &Cancellation::default(),
                &mut |_| panic!("unanchored generation")
            )
            .is_err()
        );
        f = f.reopen();
        assert_eq!(fs::read_dir(root).unwrap().count(), before);
        let saved = Journal::load(&f.owner).unwrap().unwrap();
        if saved.generation.is_some() {
            assert!(matches!(
                source::prepare(&mut f.owner, &request.project, &repo, "HEAD", &Cancellation::default()),
                Err(ReservationError::Pending)
            ));
        } else {
            source::prepare(&mut f.owner, &request.project, &repo, "HEAD", &Cancellation::default()).unwrap();
        }
    }
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
        let mut request = reservation(&f, name, 8000 + u16::try_from(index).unwrap());
        request.capabilities.agents = [Agent::Codex, Agent::Claude].into();
        f = native_prepare(f, &target, &runner, &directory, &request, 1);
        let repo = directory.join(format!("repo-{name}"));
        repository(&repo, name);
        let revision = sessions::extend_fixture(&repo);
        let descriptor = source::prepare(&mut f.owner, &request.project, &repo, "HEAD", &cancellation).unwrap();
        let saved = Journal::load(&f.owner).unwrap().unwrap();
        let root = f.owner.artifact_root().unwrap().to_owned();
        let exchange = |connection: &Connection, command: &str, bytes: &[u8]| -> reservations::Result<Vec<u8>> {
            let mut command_line = connection.pinned_command(command);
            Ok(if command.ends_with("import-project-source") {
                runner.private_file_exchange(
                    &mut command_line,
                    source::for_request(&saved, bytes)?.frame(&root, bytes, &cancellation, startup_deadline())?,
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
        f = native_sessions(f, &target, &runner, &directory, &request, &revision, index);
        f = sessions::native_prepared(f, &target, &runner, &directory, &request, index);
        projects.push((request, source_root));
    }
    let before: Vec<_> = projects
        .iter()
        .map(|(_, root)| fs::read(root.join("repository.git/HEAD")).unwrap())
        .collect();
    let sessions = Journal::load(&f.owner).unwrap().unwrap().manifest.members;
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
    let after = Journal::load(&f.owner).unwrap().unwrap().manifest;
    assert_eq!(after.members[0].sessions, sessions[0].sessions);
    assert_eq!(&after.members[1..], &sessions[1..]);
    assert_bootstrap_fenced(&mut f);
}

fn native_sessions(
    mut f: CoordinatorFixture,
    target: &crate::cloud_runtime::bootstrap_recovery::Target,
    runner: &Runner<'_>,
    directory: &Path,
    request: &Reservation,
    revision: &str,
    index: usize,
) -> CoordinatorFixture {
    let mut saved = None;
    for (agent_index, agent) in [Agent::Codex, Agent::Claude].into_iter().enumerate() {
        let change = Change::ReserveSession(request.project.clone(), Session::new(agent, revision.into()));
        let vault = f.vault.clone();
        let mut wire = Vec::new();
        let response = reservations::coordinate(
            &mut f.owner,
            target,
            &request.image_digest,
            &change,
            &mut |connection, command, bytes| {
                assert_eq!(command, "horizon-cloud-worker reserve-project-session");
                wire = bytes.to_vec();
                let reply =
                    runner.private_exchange(&mut connection.pinned_command(command), bytes, Duration::from_secs(90))?;
                if agent_index == 0 && index == 0 {
                    return Err(ReservationError::Invalid);
                }
                if agent_index == 0 && index == 2 {
                    vault.fail_after(Some(0));
                }
                Ok(reply)
            },
        );
        if agent_index == 0 && index != 1 {
            assert!(response.is_err());
            fs::write(directory.join("restart"), "session reservation recovery").unwrap();
            f = f.reopen();
            reservations::coordinate(
                &mut f.owner,
                target,
                &request.image_digest,
                &Change::Resume,
                &mut |connection, command, bytes| {
                    assert_eq!(bytes, wire);
                    Ok(runner.private_exchange(
                        &mut connection.pinned_command(command),
                        bytes,
                        Duration::from_secs(90),
                    )?)
                },
            )
            .unwrap();
        } else {
            response.unwrap();
        }
        if agent_index == 0 {
            saved = Some((change, wire));
        }
    }
    let (change, wire) = saved.unwrap();
    reservations::coordinate(
        &mut f.owner,
        target,
        &request.image_digest,
        &change,
        &mut |connection, command, bytes| {
            assert_eq!(bytes, wire);
            Ok(runner.private_exchange(&mut connection.pinned_command(command), bytes, Duration::from_secs(90))?)
        },
    )
    .unwrap();
    let journal = Journal::load(&f.owner).unwrap().unwrap();
    assert_eq!(journal.manifest.members[index].sessions.len(), 2);
    f
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

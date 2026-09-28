//! Committed source: validated and packed before allocation, transferred to the ready worker.
use super::{Connection, Deployment, Event, Result, Runner, Stage, Store, WorkerContract, repository};
use horizon_cloud::SubmoduleHistory;
use std::path::{Path, PathBuf};

pub(super) struct Packed {
    pack: PathBuf,
    auxiliary: Option<Auxiliary>,
    /// Same-worker siblings in layering order, each packed at its pinned revision.
    siblings: Vec<Sibling>,
    manifest: Option<String>,
}

struct Sibling {
    alias: String,
    revision: String,
    pack: PathBuf,
    auxiliary: Auxiliary,
}

/// A repository's source material archive. Pinned submodule history, which its committed
/// `.horizon/cloud.yml` may select, waits for the image's contract, so an image that
/// cannot record shallow submodules gets full history before any worker is allocated.
struct Auxiliary {
    archive: Option<PathBuf>,
    repository: PathBuf,
    revision: String,
    root: PathBuf,
    history: SubmoduleHistory,
}

impl Auxiliary {
    fn pack(repository: &Path, revision: &str, root: &Path, runner: &Runner<'_>) -> Result<Self> {
        let history = repository::launch::committed_config(repository, revision, runner)?
            .map(|config| config.source.submodule_history)
            .unwrap_or_default();
        let mut auxiliary = Self {
            archive: None,
            repository: repository.to_owned(),
            revision: revision.to_owned(),
            root: root.to_owned(),
            history,
        };
        if history == SubmoduleHistory::Full {
            auxiliary.settle(&WorkerContract::default(), runner)?;
        }
        Ok(auxiliary)
    }

    /// Packs a waiting archive: pinned when `contract` records shallow submodules.
    fn settle(&mut self, contract: &WorkerContract, runner: &Runner<'_>) -> Result<()> {
        if self.archive.is_none() {
            if !contract.pinned_submodules {
                self.history = SubmoduleHistory::Full;
            }
            let archive = repository::auxiliary(&self.repository, &self.revision, &self.root, self.history, runner)?;
            self.archive = Some(archive);
        }
        Ok(())
    }

    /// The archive for the ready worker. One packed pinned for an image whose contract
    /// differs from the worker's is packed again with full history; this only guards a
    /// mismatch, since [`Packed::settle`] decides from the image before allocation.
    fn for_worker(mut self, contract: &WorkerContract, runner: &Runner<'_>, emit: &dyn Fn(Event)) -> Result<PathBuf> {
        if let Some(archive) = &self.archive
            && self.history == SubmoduleHistory::Pinned
            && !contract.pinned_submodules
        {
            emit(Event::Output(
                "This worker cannot record pinned submodule history although its image could; sending full submodule history".into(),
            ));
            std::fs::remove_file(archive)?;
            self.archive = None;
        }
        self.settle(contract, runner)?;
        self.archive
            .ok_or(super::Error::Invalid("Source material was not packed"))
    }
}

impl Packed {
    /// Packs each archive that waited for `image`, the contract of the image the worker
    /// will run, before the worker is allocated.
    pub(super) fn settle(&mut self, image: &WorkerContract, runner: &Runner<'_>) -> Result<()> {
        let siblings = self.siblings.iter_mut().map(|sibling| &mut sibling.auxiliary);
        for auxiliary in self.auxiliary.iter_mut().chain(siblings) {
            auxiliary.settle(image, runner)?;
        }
        Ok(())
    }
}

pub(super) fn pack(state: &Deployment, pack_root: &Path, runner: &Runner<'_>) -> Result<Packed> {
    let pack = pack_root.join("source.pack");
    let mut auxiliary = None;
    let mut siblings = Vec::new();
    let mut manifest = None;
    if !state.source_ready {
        manifest = state
            .siblings
            .as_ref()
            .map(super::siblings::Set::manifest)
            .transpose()?;
        repository::validate_tree(&state.repository, &state.revision, runner)?;
        repository::pack(&state.repository, &state.revision, &pack, runner)?;
        auxiliary = Some(Auxiliary::pack(&state.repository, &state.revision, pack_root, runner)?);
        for (index, sibling) in state.siblings.iter().flat_map(|set| &set.members).enumerate() {
            let checkout = sibling.checkout()?;
            let root = pack_root.join(format!("sibling-{index}"));
            std::fs::create_dir(&root)?;
            let pack = root.join("source.pack");
            repository::validate_tree(checkout, &sibling.revision, runner)?;
            repository::pack(checkout, &sibling.revision, &pack, runner)?;
            siblings.push(Sibling {
                alias: sibling.alias.clone(),
                revision: sibling.revision.clone(),
                pack,
                auxiliary: Auxiliary::pack(checkout, &sibling.revision, &root, runner)?,
            });
        }
    }
    Ok(Packed {
        pack,
        auxiliary,
        siblings,
        manifest,
    })
}

/// Imports the primary and then each sibling into its own repository, and records the
/// siblings last, so no session sees a manifest naming source the worker lacks. Every
/// import replays for the same revision, so an interrupted transfer is repeated whole.
pub(super) fn transfer(
    connection: &Connection,
    store: &Store,
    state: &mut Deployment,
    packed: Packed,
    contract: &WorkerContract,
    runner: &Runner<'_>,
    emit: &dyn Fn(Event),
) -> Result<()> {
    if !state.source_ready {
        state.stage = Stage::Worktrees;
        store.save(state)?;
        emit(Event::stage(state.stage));
        connection.transfer(&packed.pack, &state.revision, runner)?;
        if let Some(auxiliary) = packed.auxiliary {
            connection.transfer_material(&auxiliary.for_worker(contract, runner, emit)?, runner)?;
        }
        for sibling in packed.siblings {
            connection.transfer_sibling(&sibling.alias, &sibling.pack, &sibling.revision, runner)?;
            let archive = sibling.auxiliary.for_worker(contract, runner, emit)?;
            connection.transfer_sibling_material(&sibling.alias, &archive, runner)?;
        }
        if let Some(manifest) = &packed.manifest {
            connection.record_siblings(manifest, runner)?;
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
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

    fn committed(path: &Path) -> String {
        std::fs::create_dir_all(path).unwrap();
        git(path, &["init", "--quiet"]);
        std::fs::write(path.join("README"), "fixture\n").unwrap();
        git(path, &["add", "."]);
        git(path, &["commit", "--quiet", "-m", "Add fixture"]);
        git(path, &["rev-parse", "HEAD"])
    }

    #[test]
    fn each_sibling_is_packed_at_its_pinned_revision_apart_from_the_primary() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        let lib = root.path().join("native-lib");
        let revision = committed(&app);
        let pinned = committed(&lib);
        git(&lib, &["commit", "--quiet", "--allow-empty", "-m", "Move on"]);
        let mut state: Deployment = serde_json::from_value(serde_json::json!({
            "version":3,"cloud_id":"siblings","repository":app,"revision":revision,
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
            "stage":"Worktrees","operation":{"state":"prepared"},"spec":null,"worker":null,"sessions":[],
            "siblings":{"primary_directory":"app","members":[{
                "alias":"native","repository":"example/native-lib","directory":"native-lib","revision":pinned,
                "local_repository":lib,"profile":"dev"
            }]}
        }))
        .unwrap();
        let cancel = Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        let packs = tempfile::tempdir().unwrap();
        let packed = pack(&state, packs.path(), &runner).unwrap();
        assert!(packed.pack.is_file());
        let [sibling] = packed.siblings.as_slice() else {
            panic!("expected one sibling");
        };
        assert_eq!(
            (sibling.alias.as_str(), sibling.revision.as_str()),
            ("native", pinned.as_str())
        );
        assert!(sibling.pack.starts_with(packs.path().join("sibling-0")) && sibling.pack.is_file());
        let archive = sibling.auxiliary.archive.as_ref().unwrap();
        assert!(archive.starts_with(packs.path().join("sibling-0")) && archive.is_file());
        git(&lib, &["index-pack", &sibling.pack.to_string_lossy()]);
        let listed = git(
            &lib,
            &[
                "verify-pack",
                "-v",
                &sibling.pack.with_extension("idx").to_string_lossy(),
            ],
        );
        assert!(listed.contains(&pinned), "the pack holds the pinned commit");
        assert!(
            !listed.contains(&git(&lib, &["rev-parse", "HEAD"])),
            "and not the later one"
        );

        state.source_ready = true;
        let again = tempfile::tempdir().unwrap();
        assert!(pack(&state, again.path(), &runner).unwrap().siblings.is_empty());
        state.source_ready = false;
        state.siblings.as_mut().unwrap().members[0].local_repository = root.path().join("moved");
        assert!(
            pack(&state, again.path(), &runner).is_err(),
            "a moved checkout is named"
        );
    }

    /// The commits in an archive's first submodule pack, read with `repository`'s Git.
    fn archived_commits(archive: &Path, repository: &Path) -> std::collections::BTreeSet<String> {
        let scratch = tempfile::tempdir().unwrap();
        let status = Command::new("tar")
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(scratch.path())
            .arg("module-0.pack")
            .status()
            .unwrap();
        assert!(status.success());
        let pack = scratch.path().join("module-0.pack");
        git(repository, &["index-pack", &pack.to_string_lossy()]);
        let listed = git(
            repository,
            &["verify-pack", "-v", &pack.with_extension("idx").to_string_lossy()],
        );
        listed
            .lines()
            .filter(|line| line.split_whitespace().nth(1) == Some("commit"))
            .map(|line| line.split_whitespace().next().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn pinned_submodule_history_is_decided_by_the_image_before_allocation() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        committed(&app);
        let module = app.join("module");
        let first = committed(&module);
        git(&module, &["commit", "--quiet", "--allow-empty", "-m", "Pin"]);
        let pinned = git(&module, &["rev-parse", "HEAD"]);
        std::fs::create_dir(app.join(".horizon")).unwrap();
        std::fs::write(
            app.join(".horizon/cloud.yml"),
            "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    \
             image: registry.example.com/horizon/dev\n    cpu: 4\n    memory_gb: 8\n\
             source:\n  submodule_history: pinned\n",
        )
        .unwrap();
        git(&app, &["add", ".horizon"]);
        git(
            &app,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{pinned},module"),
            ],
        );
        git(&app, &["commit", "--quiet", "-m", "Pin module"]);
        let revision = git(&app, &["rev-parse", "HEAD"]);
        let cancel = Cancellation::default();
        let notes = std::cell::RefCell::new(Vec::new());
        let archived = std::cell::Cell::new(0);
        let emit = |event| match event {
            Event::Output(line) if line.contains("full submodule history") => notes.borrow_mut().push(line),
            Event::Progress(progress) if progress.detail == "Pack source dependencies" => {
                archived.set(archived.get() + 1);
            }
            _ => {}
        };
        let runner = Runner {
            cancel: &cancel,
            emit: &emit,
            secrets: Vec::new(),
        };
        let current = WorkerContract {
            pinned_submodules: true,
            ..WorkerContract::default()
        };
        let older = WorkerContract::default();
        let (only_pinned, full) = ([pinned.clone()].into(), [first.clone(), pinned.clone()].into());
        // The image decides before allocation; the ready worker only guards a mismatch,
        // and a reconnect without an image check decides from the worker once.
        for (image, worker, expected, repacked) in [
            (Some(&current), &current, &only_pinned, false),
            (Some(&older), &older, &full, false),
            (Some(&current), &older, &full, true),
            (None, &current, &only_pinned, false),
        ] {
            notes.borrow_mut().clear();
            let packs = tempfile::tempdir().unwrap();
            let mut packed = Packed {
                pack: packs.path().join("source.pack"),
                auxiliary: Some(Auxiliary::pack(&app, &revision, packs.path(), &runner).unwrap()),
                siblings: Vec::new(),
                manifest: None,
            };
            assert!(
                packed.auxiliary.as_ref().unwrap().archive.is_none(),
                "pinned waits for a contract"
            );
            if let Some(image) = image {
                packed.settle(image, &runner).unwrap();
                assert!(packed.auxiliary.as_ref().unwrap().archive.as_ref().unwrap().is_file());
            }
            archived.set(0);
            let archive = packed.auxiliary.unwrap().for_worker(worker, &runner, &emit).unwrap();
            assert_eq!(&archived_commits(&archive, &module), expected);
            let packed_after_ready = usize::from(repacked || image.is_none());
            assert_eq!(
                archived.get(),
                packed_after_ready,
                "archives packed once the worker is ready"
            );
            assert_eq!(notes.borrow().len(), usize::from(repacked));
        }
    }
}

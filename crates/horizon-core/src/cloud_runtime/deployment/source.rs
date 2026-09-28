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

/// A repository's source material archive, packed with the submodule history its
/// committed `.horizon/cloud.yml` selects, and what it takes to pack it again.
struct Auxiliary {
    archive: PathBuf,
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
        Ok(Self {
            archive: repository::auxiliary(repository, revision, root, history, runner)?,
            repository: repository.to_owned(),
            revision: revision.to_owned(),
            root: root.to_owned(),
            history,
        })
    }

    /// A worker image that cannot record a shallow submodule gets full history instead.
    fn for_worker(self, contract: &WorkerContract, runner: &Runner<'_>, emit: &dyn Fn(Event)) -> Result<PathBuf> {
        if self.history == SubmoduleHistory::Full || contract.pinned_submodules {
            return Ok(self.archive);
        }
        emit(Event::Output(
            "This worker image predates pinned submodule history; sending full submodule history".into(),
        ));
        std::fs::remove_file(&self.archive)?;
        repository::auxiliary(
            &self.repository,
            &self.revision,
            &self.root,
            SubmoduleHistory::Full,
            runner,
        )
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
        let archive = &sibling.auxiliary.archive;
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
    fn a_repository_can_opt_into_pinned_submodule_history_and_older_workers_get_full() {
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
        let emit = |event| {
            if let Event::Output(line) = event
                && line.contains("full submodule history")
            {
                notes.borrow_mut().push(line);
            }
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
        for (contract, expected) in [
            (&current, [pinned.clone()].into()),
            (&WorkerContract::default(), [first.clone(), pinned.clone()].into()),
        ] {
            let packs = tempfile::tempdir().unwrap();
            let auxiliary = Auxiliary::pack(&app, &revision, packs.path(), &runner).unwrap();
            assert_eq!(auxiliary.history, SubmoduleHistory::Pinned);
            let archive = auxiliary.for_worker(contract, &runner, &emit).unwrap();
            assert_eq!(archived_commits(&archive, &module), expected);
        }
        assert_eq!(notes.borrow().len(), 1, "only the older worker is told why");
    }
}

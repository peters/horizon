//! Committed source: validated and packed before allocation, transferred to the ready worker.
use super::{Connection, Deployment, Event, Result, Runner, Stage, Store, WorkerContract, repository};
use horizon_cloud::{Lfs, Source, SubmoduleHistory};
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

/// A repository's source material archive. Pinned submodule history and an LFS path
/// selection, which its committed `.horizon/cloud.yml` may ask for, wait for the image's
/// contract, so an image that cannot honor them gets everything before any worker is
/// allocated.
struct Auxiliary {
    archive: Option<PathBuf>,
    repository: PathBuf,
    revision: String,
    root: PathBuf,
    /// As configured until packed, then as packed.
    source: Source,
}

/// What of `configured` a worker reporting `contract` can receive.
fn supported(configured: &Source, contract: &WorkerContract) -> Source {
    Source {
        submodule_history: if contract.pinned_submodules {
            configured.submodule_history
        } else {
            SubmoduleHistory::Full
        },
        lfs: if contract.lfs_selection {
            configured.lfs.clone()
        } else {
            Lfs::default()
        },
    }
}

/// What a worker supporting only `supported` of the `packed` packaging receives instead.
fn fallback_note(packed: &Source, supported: &Source) -> Option<String> {
    let mut dropped = Vec::new();
    if packed.submodule_history != supported.submodule_history {
        dropped.push("full submodule history");
    }
    if packed.lfs != supported.lfs {
        dropped.push("every LFS object");
    }
    (!dropped.is_empty()).then(|| {
        format!(
            "This worker supports less source packaging than its image reported; sending {}",
            dropped.join(" and ")
        )
    })
}

impl Auxiliary {
    fn pack(repository: &Path, revision: &str, root: &Path, runner: &Runner<'_>) -> Result<Self> {
        let source = repository::launch::committed_config(repository, revision, runner)?
            .map(|config| config.source)
            .unwrap_or_default();
        let mut auxiliary = Self {
            archive: None,
            repository: repository.to_owned(),
            revision: revision.to_owned(),
            root: root.to_owned(),
            source,
        };
        // Either way every source asset the transfer may need is verified before allocation.
        if auxiliary.source.is_default() {
            auxiliary.settle(&WorkerContract::default(), runner)?;
        } else {
            repository::validate_selected(repository, revision, &auxiliary.source.lfs, runner)?;
        }
        Ok(auxiliary)
    }

    /// Packs a waiting archive with what `contract` supports of the configured packaging.
    fn settle(&mut self, contract: &WorkerContract, runner: &Runner<'_>) -> Result<()> {
        if self.archive.is_none() {
            self.source = supported(&self.source, contract);
            let archive = repository::auxiliary(&self.repository, &self.revision, &self.root, &self.source, runner)?;
            self.archive = Some(archive);
        }
        Ok(())
    }

    /// The archive for the ready worker. One packed for an image whose contract promised
    /// more than the worker reports is packed again; this only guards a mismatch, since
    /// [`Packed::settle`] decides from the image before allocation.
    fn for_worker(mut self, contract: &WorkerContract, runner: &Runner<'_>, emit: &dyn Fn(Event)) -> Result<PathBuf> {
        if let Some(archive) = &self.archive
            && let Some(note) = fallback_note(&self.source, &supported(&self.source, contract))
        {
            emit(Event::Output(note));
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
        auxiliary = Some(Auxiliary::pack(&state.repository, &state.revision, pack_root, runner)?);
        repository::pack(&state.repository, &state.revision, &pack, runner)?;
        for (index, sibling) in state.siblings.iter().flat_map(|set| &set.members).enumerate() {
            let checkout = sibling.checkout()?;
            let root = pack_root.join(format!("sibling-{index}"));
            std::fs::create_dir(&root)?;
            let pack = root.join("source.pack");
            let auxiliary = Auxiliary::pack(checkout, &sibling.revision, &root, runner)?;
            repository::pack(checkout, &sibling.revision, &pack, runner)?;
            siblings.push(Sibling {
                alias: sibling.alias.clone(),
                revision: sibling.revision.clone(),
                pack,
                auxiliary,
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
    fn source_packaging_is_decided_by_the_image_before_allocation() {
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
             source:\n  submodule_history: pinned\n  lfs: {exclude: ['fixtures/**']}\n",
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
            lfs_selection: true,
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
            let manifest = Command::new("tar")
                .arg("-xOf")
                .arg(&archive)
                .arg("manifest.json")
                .output()
                .unwrap();
            let manifest: serde_json::Value = serde_json::from_slice(&manifest.stdout).unwrap();
            assert_eq!(
                manifest.get("lfs").is_some(),
                expected == &only_pinned,
                "the selection travels with pinned history"
            );
            let packed_after_ready = usize::from(repacked || image.is_none());
            assert_eq!(
                archived.get(),
                packed_after_ready,
                "archives packed once the worker is ready"
            );
            assert_eq!(notes.borrow().len(), usize::from(repacked));
        }
    }

    #[test]
    fn the_fallback_note_names_exactly_what_the_worker_does_not_support() {
        let packed: Source =
            serde_yaml::from_str("submodule_history: pinned\nlfs: {exclude: ['fixtures/**']}").unwrap();
        let contract = |pinned_submodules, lfs_selection| WorkerContract {
            pinned_submodules,
            lfs_selection,
            ..WorkerContract::default()
        };
        let note = |worker| fallback_note(&packed, &supported(&packed, &worker));
        let prefix = "This worker supports less source packaging than its image reported; sending ";
        assert_eq!(note(contract(true, true)), None);
        assert_eq!(
            note(contract(false, true)),
            Some(format!("{prefix}full submodule history"))
        );
        assert_eq!(note(contract(true, false)), Some(format!("{prefix}every LFS object")));
        assert_eq!(
            note(contract(false, false)),
            Some(format!("{prefix}full submodule history and every LFS object"))
        );
    }
}

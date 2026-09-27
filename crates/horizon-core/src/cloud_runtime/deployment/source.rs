//! Committed source: validated and packed before allocation, transferred to the ready worker.
use super::{Connection, Deployment, Event, Result, Runner, Stage, Store, repository};
use std::path::{Path, PathBuf};

pub(super) struct Packed {
    pack: PathBuf,
    auxiliary: Option<PathBuf>,
    /// Same-worker siblings in layering order, each packed at its pinned revision.
    siblings: Vec<Sibling>,
    manifest: Option<String>,
}

struct Sibling {
    alias: String,
    revision: String,
    pack: PathBuf,
    auxiliary: PathBuf,
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
        auxiliary = Some(repository::auxiliary(
            &state.repository,
            &state.revision,
            pack_root,
            runner,
        )?);
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
                auxiliary: repository::auxiliary(checkout, &sibling.revision, &root, runner)?,
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
    runner: &Runner<'_>,
    emit: &dyn Fn(Event),
) -> Result<()> {
    if !state.source_ready {
        state.stage = Stage::Worktrees;
        store.save(state)?;
        emit(Event::stage(state.stage));
        connection.transfer(&packed.pack, &state.revision, runner)?;
        if let Some(auxiliary) = packed.auxiliary {
            connection.transfer_material(&auxiliary, runner)?;
        }
        for sibling in &packed.siblings {
            connection.transfer_sibling(&sibling.alias, &sibling.pack, &sibling.revision, runner)?;
            connection.transfer_sibling_material(&sibling.alias, &sibling.auxiliary, runner)?;
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
        assert!(sibling.auxiliary.starts_with(packs.path().join("sibling-0")) && sibling.auxiliary.is_file());
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
}

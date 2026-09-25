use super::*;
use crate::bootstrap::{membership::mutate_with, namespaces::Boundary};
use std::os::unix::fs::{MetadataExt, symlink};

fn project_root(f: &Fixture, project: &ProjectIdentity) -> std::path::PathBuf {
    f.directory
        .path()
        .join("workspace/projects")
        .join(project.project_id().to_string())
}
fn prepare(f: &Fixture, project: &ProjectIdentity) -> RecoveryRequest {
    f.membership(
        project,
        f.manifest().revision,
        OperationId::generate(),
        &Request::PrepareNamespace {},
    )
}
fn reserved(f: &Fixture, name: &str, port: u16) -> ProjectIdentity {
    let project = identity(name);
    f.change(&f.membership(
        &project,
        f.manifest().revision,
        OperationId::generate(),
        &reserve(port, false),
    ))
    .unwrap();
    project
}
fn boot(f: &Fixture) -> io::Result<()> {
    let store = Store::open(&f.root())?;
    let bootstrap = decode(&store.read(BOOTSTRAP)?.unwrap())?;
    startup(&store, &bootstrap)
}
fn cancel(f: &Fixture, project: &ProjectIdentity) -> RecoveryRequest {
    f.membership(
        project,
        f.manifest().revision,
        OperationId::generate(),
        &Request::Cancel {},
    )
}
fn injected(
    f: &Fixture,
    request: &RecoveryRequest,
    boundary: &mut impl FnMut(Boundary) -> io::Result<()>,
) -> io::Result<Receipt> {
    mutate_with(
        &Store::open(&f.root())?,
        &f.runtime,
        request,
        Action::ReconcileProject,
        |_| Ok(()),
        &mut |_| Ok(()),
        boundary,
    )
}

#[test]
fn three_namespaces_keep_data_and_siblings_after_terminal_cancellation() {
    let f = Fixture::ready();
    let mut projects = Vec::new();
    for (index, name) in ["one", "two", "three"].iter().enumerate() {
        let project = reserved(&f, name, 8000 + u16::try_from(index).unwrap());
        let request = prepare(&f, &project);
        let receipt = f.change(&request).unwrap();
        assert_eq!(receipt.state, State::Preparing);
        let root = project_root(&f, &project);
        for child in ["repository", "worktrees", "homes", "runtime", "logs", "tools"] {
            assert_eq!(fs::metadata(root.join(child)).unwrap().mode() & 0o777, 0o700);
        }
        fs::write(root.join("repository/dirty"), name).unwrap();
        let inode = fs::metadata(&root).unwrap().ino();
        assert_eq!(f.change(&request).unwrap(), receipt);
        assert_eq!(fs::metadata(&root).unwrap().ino(), inode);
        projects.push((project, request, inode));
    }
    boot(&f).unwrap();
    let before = f.manifest();
    let removed = cancel(&f, &projects[0].0);
    assert_eq!(f.change(&removed).unwrap().state, State::Removed);
    assert!(f.change(&projects[0].1).is_err());
    assert!(f.change(&prepare(&f, &projects[0].0)).is_err());
    assert_eq!(&f.manifest().members[1..], &before.members[1..]);
    for ((project, _, inode), expected) in projects.iter().zip(["one", "two", "three"]) {
        let root = project_root(&f, project);
        assert_eq!(fs::metadata(&root).unwrap().ino(), *inode);
        assert_eq!(fs::read_to_string(root.join("repository/dirty")).unwrap(), expected);
    }
    boot(&f).unwrap();
    f.change(&removed).unwrap();
    reserved(&f, "replacement", 8000);
}

#[test]
fn anchored_interruption_boundaries_resume_without_replacing_roots() {
    for boundary in [
        Boundary::Anchored,
        Boundary::Child,
        Boundary::Populated,
        Boundary::Published,
        Boundary::Synced,
    ] {
        for target in [1, 2] {
            if boundary == Boundary::Child && target == 1 {
                continue;
            }
            let f = Fixture::ready();
            let project = reserved(&f, "one", 8000);
            let request = prepare(&f, &project);
            let mut tree = 0;
            assert!(
                injected(&f, &request, &mut |at| {
                    if at == Boundary::Anchored {
                        tree += 1;
                    }
                    if tree == target && at == boundary {
                        return Err(io::Error::other("injected namespace interruption"));
                    }
                    Ok(())
                })
                .is_err(),
                "{target} {boundary:?}"
            );
            boot(&f).unwrap();
            let pending = f.manifest();
            assert_eq!(pending.members[0].state, State::Preparing);
            assert!(f.change(&cancel(&f, &project)).is_err());
            let before = fs::metadata(project_root(&f, &project)).ok().map(|meta| meta.ino());
            let receipt = f.change(&request).unwrap();
            assert_eq!(receipt.revision, pending.revision);
            if let Some(inode) = before {
                assert_eq!(fs::metadata(project_root(&f, &project)).unwrap().ino(), inode);
            }
            assert_eq!(f.change(&request).unwrap(), receipt);
            boot(&f).unwrap();
        }
    }
}

#[test]
fn directory_without_durable_inode_anchor_remains_uncertain() {
    for target in [1, 2] {
        let f = Fixture::ready();
        let project = reserved(&f, "one", 8000);
        let request = prepare(&f, &project);
        let mut created = 0;
        assert!(
            injected(&f, &request, &mut |at| {
                if at == Boundary::Created {
                    created += 1;
                }
                if created == target {
                    return Err(io::Error::other("unanchored creation"));
                }
                Ok(())
            })
            .is_err()
        );
        assert!(f.change(&request).is_err());
        assert!(boot(&f).is_err());
        assert!(f.change(&cancel(&f, &project)).is_err());
        assert_eq!(f.manifest().members[0].state, State::Preparing);
    }
}

#[test]
fn foreign_roots_symlinks_and_changed_anchor_cannot_be_adopted() {
    for variant in 0..5 {
        let f = Fixture::ready();
        let sibling = reserved(&f, "sibling", 8000);
        f.change(&prepare(&f, &sibling)).unwrap();
        let sibling_file = project_root(&f, &sibling).join("repository/dirty");
        fs::write(&sibling_file, "keep").unwrap();
        let project = reserved(&f, "other", 8001);
        let request = prepare(&f, &project);
        let root = project_root(&f, &project);
        match variant {
            0 => {
                fs::create_dir(&root).unwrap();
            }
            1 => {
                symlink(project_root(&f, &sibling), &root).unwrap();
            }
            _ => {
                f.change(&request).unwrap();
                match variant {
                    2 => {
                        fs::write(root.join(".namespace-owner.json"), b"{}").unwrap();
                    }
                    3 => {
                        fs::rename(&root, root.with_extension("retained")).unwrap();
                        fs::create_dir(&root).unwrap();
                    }
                    _ => {
                        fs::remove_dir(root.join("homes")).unwrap();
                        symlink(project_root(&f, &sibling), root.join("homes")).unwrap();
                    }
                }
            }
        }
        assert!(f.change(&request).is_err());
        assert_eq!(fs::read_to_string(&sibling_file).unwrap(), "keep");
        assert!(f.change(&cancel(&f, &project)).is_err());
    }
}

#[test]
fn published_namespace_never_recreates_missing_children_or_a_missing_root() {
    for root_missing in [false, true] {
        let f = Fixture::ready();
        let project = reserved(&f, "one", 8000);
        let request = prepare(&f, &project);
        f.change(&request).unwrap();
        let root = project_root(&f, &project);
        let missing = if root_missing { root.clone() } else { root.join("homes") };
        fs::rename(&missing, missing.with_extension("retained")).unwrap();
        assert!(f.change(&request).is_err());
        assert!(!missing.exists());
        assert!(boot(&f).is_err());
    }
}

#[test]
fn namespace_replacement_during_publication_cannot_be_acknowledged() {
    let f = Fixture::ready();
    let project = reserved(&f, "one", 8000);
    let request = prepare(&f, &project);
    let mut tree = 0;
    assert!(
        injected(&f, &request, &mut |at| {
            if at == Boundary::Anchored {
                tree += 1;
            }
            if tree == 2 && at == Boundary::Published {
                let root = project_root(&f, &project);
                fs::rename(&root, root.with_extension("retained"))?;
                fs::create_dir(&root)?;
            }
            Ok(())
        })
        .is_err()
    );
    assert!(f.change(&request).is_err());
    assert_eq!(f.manifest().members[0].state, State::Preparing);
}

#[test]
fn namespace_preparation_reserves_capacity_for_every_eventual_cancellation() {
    let f = Fixture::ready();
    let mut manifest = f.manifest();
    let mut projects = Vec::new();
    for index in 0..32 {
        let project = identity(&format!("project-{index}"));
        let request = f.membership(
            &project,
            manifest.revision,
            OperationId::generate(),
            &reserve(8000 + index, false),
        );
        let Ok((next, _)) = manifest.next(&request.message, &request.payload) else {
            break;
        };
        manifest = next;
        let request = f.membership(
            &project,
            manifest.revision,
            OperationId::generate(),
            &Request::PrepareNamespace {},
        );
        if let Ok((next, _)) = manifest.next(&request.message, &request.payload) {
            manifest = next;
        }
        projects.push(project);
        assert!(manifest.operations.len() + projects.len() <= 64);
    }
    assert!(!projects.is_empty() && projects.len() < 32);
    for project in projects {
        let request = f.membership(
            &project,
            manifest.revision,
            OperationId::generate(),
            &Request::Cancel {},
        );
        manifest = manifest.next(&request.message, &request.payload).unwrap().0;
    }
    assert!(manifest.members.iter().all(|member| member.state == State::Removed));
    manifest.validate().unwrap();
}

#[test]
fn changed_namespace_at_cancellation_publication_never_acknowledges() {
    for point in [Publication::Staged, Publication::Renamed, Publication::Durable] {
        for change_header in [false, true] {
            let f = Fixture::ready();
            let project = reserved(&f, "one", 8000);
            f.change(&prepare(&f, &project)).unwrap();
            let root = project_root(&f, &project);
            fs::write(root.join("repository/dirty"), "keep").unwrap();
            let request = cancel(&f, &project);
            assert!(
                mutate(
                    &Store::open(&f.root()).unwrap(),
                    &f.runtime,
                    &request,
                    Action::RemoveProject,
                    |_| Ok(()),
                    &mut |at| {
                        if at == point {
                            if change_header {
                                fs::write(root.join(".namespace-owner.json"), b"{}")?;
                            } else {
                                fs::rename(&root, root.with_extension("retained"))?;
                                fs::create_dir(&root)?;
                            }
                        }
                        Ok(())
                    }
                )
                .is_err()
            );
            assert!(f.change(&request).is_err());
            let retained = if change_header {
                root.clone()
            } else {
                root.with_extension("retained")
            };
            assert_eq!(fs::read_to_string(retained.join("repository/dirty")).unwrap(), "keep");
        }
    }
}

#[test]
fn startup_rechecks_staged_and_published_container_identity() {
    for published in [false, true] {
        let f = Fixture::ready();
        let project = reserved(&f, "one", 8000);
        let request = prepare(&f, &project);
        if published {
            f.change(&request).unwrap();
        } else {
            assert!(
                injected(&f, &request, &mut |at| {
                    if at == Boundary::Anchored {
                        return Err(io::Error::other("stop at container anchor"));
                    }
                    Ok(())
                })
                .is_err()
            );
        }
        let path = if published {
            f.directory.path().join("workspace/projects")
        } else {
            f.root().join(".namespaces.next")
        };
        assert!(
            crate::bootstrap::namespaces::validate_with(&Store::open(&f.root()).unwrap(), &f.manifest(), || {
                fs::rename(&path, path.with_extension("retained"))?;
                fs::create_dir(&path)?;
                Ok(())
            })
            .is_err()
        );
        assert!(path.with_extension("retained").is_dir());
    }
}

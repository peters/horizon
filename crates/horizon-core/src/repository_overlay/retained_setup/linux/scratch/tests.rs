use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

// Real confined I/O and synchronization, but deliberately no filesystem qualifier:
// these are state/fault tests, not power-loss or durable-admission evidence.
fn parent() -> (tempfile::TempDir, Directory) {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("root");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let directory = Directory {
        reader: SelectedRepositoryReader::open(&path).unwrap().root,
        handle: File::open(&path).unwrap(),
        path,
    };
    (fixture, directory)
}

#[test]
fn fresh_scratch_syncs_child_then_parent_and_drop_retains_it() {
    let (_fixture, parent) = parent();
    let mut calls = 0;
    let scratch = create_with(&parent, &mut reserve, &mut open, &mut |file| {
        calls += 1;
        let expected = if calls == 1 {
            parent.path.join(SCRATCH_NAME)
        } else {
            parent.path.clone()
        };
        assert_eq!(file.metadata()?.ino(), fs::metadata(expected)?.ino());
        file.sync_all()
    })
    .unwrap();
    assert_eq!(calls, 2);
    assert_eq!(fs::metadata(scratch.path()).unwrap().mode() & 0o7777, 0o700);
    assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 0);
    drop(scratch);
    assert!(parent.path.join(SCRATCH_NAME).is_dir());
}

#[test]
fn existing_nodes_are_never_opened_adopted_or_replaced() {
    for kind in 0..4 {
        let (fixture, parent) = parent();
        let path = parent.path.join(SCRATCH_NAME);
        let sentinel = fixture.path().join("sentinel");
        fs::write(&sentinel, b"unchanged").unwrap();
        match kind {
            0 => fs::write(&path, b"existing").unwrap(),
            1 => fs::create_dir(&path).unwrap(),
            2 => symlink(&sentinel, &path).unwrap(),
            _ => rustix::fs::mkfifoat(&parent.handle, SCRATCH_NAME, Mode::RUSR | Mode::WUSR).unwrap(),
        }
        let inode = fs::symlink_metadata(&path).unwrap().ino();
        let result = create_with(&parent, &mut reserve, &mut |_| panic!("open existing"), &mut |_| {
            panic!("sync existing")
        });
        assert!(matches!(result, Err(Error::ExistingScratch)));
        assert_eq!(fs::symlink_metadata(path).unwrap().ino(), inode);
        assert_eq!(fs::read(sentinel).unwrap(), b"unchanged");
    }
}

#[test]
fn create_open_and_sync_faults_retain_every_named_residue() {
    for fail in 0..5 {
        let (_fixture, parent) = parent();
        let mut syncs = 0;
        let result = create_with(
            &parent,
            &mut |file| {
                if fail == 0 {
                    return Err(rustix::io::Errno::IO);
                }
                reserve(file)?;
                if fail == 1 {
                    return Err(rustix::io::Errno::IO);
                }
                Ok(())
            },
            &mut |file| {
                if fail == 2 {
                    return Err(rustix::io::Errno::IO);
                }
                open(file)
            },
            &mut |file| {
                syncs += 1;
                if fail == syncs + 2 {
                    return Err(io::ErrorKind::Other.into());
                }
                file.sync_all()
            },
        );
        assert!(matches!(result, Err(Error::Storage)));
        assert_eq!(parent.path.join(SCRATCH_NAME).exists(), fail != 0);
        assert_eq!(syncs, if fail < 3 { 0 } else { fail - 2 });
    }
}

#[test]
fn root_replacement_after_creation_keeps_residue_in_original_root() {
    let (fixture, parent) = parent();
    let moved = fixture.path().join("original");
    let result = create_with(
        &parent,
        &mut reserve,
        &mut |file| {
            let child = open(file)?;
            fs::rename(&parent.path, &moved).unwrap();
            fs::create_dir(&parent.path).unwrap();
            fs::set_permissions(&parent.path, fs::Permissions::from_mode(0o700)).unwrap();
            Ok(child)
        },
        &mut |_| panic!("sync rebound root"),
    );
    assert!(matches!(result, Err(Error::UnsafeRoot)));
    assert!(moved.join(SCRATCH_NAME).is_dir());
    assert!(!parent.path.join(SCRATCH_NAME).exists());
}

#[test]
fn insecure_or_replaced_scratch_never_acknowledges_execution() {
    for replace in [false, true] {
        let (_fixture, parent) = parent();
        let path = parent.path.join(SCRATCH_NAME);
        let result = create_with(&parent, &mut reserve, &mut open, &mut |file| {
            file.sync_all()?;
            if file.metadata()?.ino() != parent.handle.metadata()?.ino() {
                if replace {
                    fs::rename(&path, parent.path.join("original-scratch"))?;
                    fs::create_dir(&path)?;
                }
                fs::set_permissions(&path, fs::Permissions::from_mode(if replace { 0o700 } else { 0o755 }))?;
            }
            Ok(())
        });
        assert!(matches!(result, Err(Error::UnsafeRoot)));
        assert!(path.is_dir());
        assert_eq!(parent.path.join("original-scratch").exists(), replace);
    }
}

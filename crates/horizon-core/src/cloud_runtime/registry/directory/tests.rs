use super::*;

#[test]
fn an_interrupted_ancestor_is_recovered_before_a_child_can_be_created() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let parent = root.join("parent");
    let child = parent.join("child");
    let error = ensure_with(&parent, true, &mut |_| Err(Error::Invalid("synthetic interruption")));
    assert!(error.is_err());
    assert!(parent.is_dir());
    assert_eq!(std::fs::metadata(marker(&root)).unwrap().len(), 1);
    let mut observed = false;
    ensure_with(&child, true, &mut |created| {
        assert_eq!(created, child);
        assert_eq!(std::fs::metadata(marker(&root)).unwrap().len(), 0);
        observed = true;
        Ok(())
    })
    .unwrap();
    assert!(observed);
    assert!(child.is_dir());
    assert_eq!(std::fs::metadata(marker(&parent)).unwrap().len(), 0);
}

struct RestorePermissions(PathBuf);
impl Drop for RestorePermissions {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
    }
}

#[test]
fn a_pre_existing_private_root_does_not_need_a_readable_parent() {
    for existing_marker in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().join("parent");
        let root = parent.join("registry");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        if existing_marker {
            drop(lock(&parent, true).unwrap());
        }
        let _restore = RestorePermissions(parent.clone());
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o300)).unwrap();
        if rustix::process::geteuid().as_raw() != 0 {
            assert!(File::open(&parent).is_err(), "the fixture parent must be unreadable");
        }
        ensure(&root).unwrap();
        assert!(root.is_dir());
        assert_eq!(metadata(&marker(&parent)).unwrap().is_some(), existing_marker);
    }
}

#[test]
fn a_coordination_symlink_cannot_modify_an_existing_private_file() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("registry");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let kept = temporary.path().join("kept");
    std::fs::write(&kept, "synthetic-kept-data").unwrap();
    std::os::unix::fs::symlink(&kept, marker(temporary.path())).unwrap();
    assert!(ensure(&root).is_err());
    assert_eq!(std::fs::read_to_string(kept).unwrap(), "synthetic-kept-data");
}

#[test]
fn a_coordination_hard_link_cannot_extend_an_existing_private_file() {
    let temporary = tempfile::tempdir().unwrap();
    let kept = temporary.path().join("kept");
    std::fs::write(&kept, b"").unwrap();
    std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::hard_link(&kept, marker(temporary.path())).unwrap();
    let root = temporary.path().join("registry");
    assert!(ensure(&root).is_err());
    assert!(!root.exists());
    assert_eq!(std::fs::metadata(kept).unwrap().len(), 0);
}

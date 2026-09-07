use super::*;

#[cfg(target_os = "linux")]
mod safety;
#[cfg(target_os = "linux")]
mod semantics;

fn source() -> GitSource {
    GitSource {
        repository: "team/repo".into(),
        commit: crate::cloud_run::GitCommitSha::parse("a".repeat(40)).unwrap(),
        branch: None,
    }
}

#[test]
fn selection_and_source_policy_fail_before_opening_any_repository() {
    let missing = Path::new("/not-a-selected-repository");
    for path in [".git/config", ".env", "dir/auth.json", "../escape", "/absolute"] {
        assert!(matches!(
            capture_selected(missing, source(), &[path]),
            Err(GitCaptureError::Policy(_))
        ));
    }
    assert_eq!(
        capture_selected(missing, source(), &["same", "same"]),
        Err(OverlayPlanError::DuplicatePath.into())
    );
    let too_many = vec!["same"; MAX_CHANGES / 2 + 1];
    assert_eq!(
        capture_selected(missing, source(), &too_many),
        Err(OverlayPlanError::ChangeLimit.into())
    );
    let mut invalid = source();
    invalid.repository = "https://user:private@example.invalid/repo".into();
    assert_eq!(
        capture_selected(missing, invalid, &[]),
        Err(OverlayPlanError::InvalidSource.into())
    );
}

#[test]
fn metadata_budget_and_diagnostics_are_bounded_and_redacted() {
    let paths: Vec<_> = (0..1100).map(|index| format!("{index}{}", "x".repeat(4000))).collect();
    let selected: Vec<_> = paths.iter().map(String::as_str).collect();
    assert_eq!(
        validate_selection(&selected),
        Err(OverlayPlanError::MetadataLimit.into())
    );
    let error = capture_selected(
        Path::new("/private-root-do-not-print"),
        source(),
        &[".env.private-value"],
    )
    .unwrap_err();
    for rendered in [format!("{error:?}"), error.to_string()] {
        assert!(!rendered.contains("private-value"));
        assert!(!rendered.contains("private-root"));
        assert!(!rendered.contains("team/repo"));
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_platform_has_no_fallback() {
    assert_eq!(
        capture_selected(Path::new("/selected"), source(), &["file"]),
        Err(GitCaptureError::Unsupported)
    );
}

#[cfg(target_os = "linux")]
struct Fixture {
    directory: tempfile::TempDir,
    repository: git2::Repository,
}

#[cfg(target_os = "linux")]
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let repository = git2::Repository::init(directory.path()).unwrap();
        let fixture = Self { directory, repository };
        fixture.commit();
        fixture
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        let selected = self.root().join(path);
        std::fs::create_dir_all(selected.parent().unwrap()).unwrap();
        std::fs::write(selected, bytes).unwrap();
    }

    fn stage(&self, path: &str, bytes: &[u8], mode: u32) {
        let mut index = self.repository.index().unwrap();
        index
            .add(&git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode,
                uid: 0,
                gid: 0,
                file_size: u32::try_from(bytes.len()).unwrap(),
                id: self.repository.blob(bytes).unwrap(),
                flags: 0,
                flags_extended: 0,
                path: path.as_bytes().to_vec(),
            })
            .unwrap();
        index.write().unwrap();
    }

    fn commit(&self) {
        let tree_id = self.repository.index().unwrap().write_tree().unwrap();
        let tree = self.repository.find_tree(tree_id).unwrap();
        let signature = git2::Signature::now("Fixture", "fixture@example.invalid").unwrap();
        let parent = self.repository.head().ok().and_then(|head| head.peel_to_commit().ok());
        let parents: Vec<_> = parent.iter().collect();
        self.repository
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "Synthetic fixture",
                &tree,
                &parents,
            )
            .unwrap();
    }

    fn source(&self) -> GitSource {
        let oid = self.repository.head().unwrap().peel_to_commit().unwrap().id();
        GitSource {
            commit: crate::cloud_run::GitCommitSha::parse(oid.to_string()).unwrap(),
            ..source()
        }
    }

    fn capture(&self, paths: &[&str]) -> RepositoryOverlayBundle {
        capture_selected(self.root(), self.source(), paths).unwrap()
    }
}

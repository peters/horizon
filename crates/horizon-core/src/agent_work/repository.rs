use std::fmt::Write;
use std::fs;
use std::io::{Read, Result};
use std::path::Path;
use std::time::{Duration, Instant};

use git2::{Repository, RepositoryState, StatusOptions};
use sha2::{Digest, Sha256};

const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// A conservative fingerprint of HEAD, index and non-ignored dirty contents.
/// Unknown, oversized, changing or unresolved repositories require confirmation.
pub(super) fn fingerprint(cwd: &Path) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let repository = Repository::discover(cwd).ok()?;
    if repository.state() != RepositoryState::Clean {
        return None;
    }
    let root = repository.workdir()?;
    let head = repository.head().ok()?.peel_to_commit().ok()?.id();
    let mut hash = Sha256::new();
    hash.update(root.canonicalize().ok()?.as_os_str().as_encoded_bytes());
    hash.update(head.as_bytes());
    let mut remaining = MAX_BYTES;
    hash_file(&repository.path().join("index"), &mut hash, &mut remaining).ok()?;
    let mut options = StatusOptions::new();
    options
        .include_ignored(false)
        .include_untracked(true)
        .recurse_untracked_dirs(true);
    let statuses = repository.statuses(Some(&mut options)).ok()?;
    if statuses.len() > 512 {
        return None;
    }
    for entry in &statuses {
        if Instant::now() >= deadline || entry.status().is_conflicted() {
            return None;
        }
        let relative = entry.path().ok()?;
        hash.update(relative.as_bytes());
        hash.update(entry.status().bits().to_le_bytes());
        let path = root.join(relative);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_symlink() => {
                hash.update(fs::read_link(path).ok()?.as_os_str().as_encoded_bytes());
            }
            Ok(metadata) if metadata.is_file() => {
                hash.update([u8::from(metadata.permissions().readonly())]);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    hash.update(metadata.permissions().mode().to_le_bytes());
                }
                hash_file(&path, &mut hash, &mut remaining).ok()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => hash.update(b"deleted"),
            _ => return None,
        }
    }
    if Instant::now() >= deadline || repository.head().ok()?.peel_to_commit().ok()?.id() != head {
        return None;
    }
    let mut result = String::with_capacity(64);
    for byte in hash.finalize() {
        write!(&mut result, "{byte:02x}").ok()?;
    }
    Some(result)
}

fn hash_file(path: &Path, hash: &mut Sha256, remaining: &mut u64) -> Result<()> {
    let mut file = fs::File::open(path)?;
    let before = file.metadata()?;
    if before.len() > *remaining {
        return Err(std::io::Error::other("repository evidence exceeds limit"));
    }
    hash.update(before.len().to_le_bytes());
    let mut reader = (&mut file).take(*remaining + 1);
    let mut buffer = [0; 8192];
    let mut bytes_read = 0;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes_read += count as u64;
        if bytes_read > *remaining {
            return Err(std::io::Error::other("repository changed during snapshot"));
        }
        hash.update(&buffer[..count]);
    }
    let after = file.metadata()?;
    if bytes_read != before.len() || before.len() != after.len() || before.modified()? != after.modified()? {
        return Err(std::io::Error::other("repository changed during snapshot"));
    }
    *remaining -= bytes_read;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_preserves_the_fingerprint_format_and_shared_byte_budget() {
        let dir = tempfile::tempdir().expect("fixture");
        let path = dir.path().join("file");
        let contents = b"fingerprint fixture\n".repeat(1024);
        fs::write(&path, &contents).expect("write");
        let size = contents.len() as u64;
        let mut expected = Sha256::new();
        expected.update(size.to_le_bytes());
        expected.update(&contents);
        let mut actual = Sha256::new();
        let mut remaining = size;
        hash_file(&path, &mut actual, &mut remaining).expect("streaming fingerprint");
        assert_eq!(actual.finalize(), expected.finalize());
        assert_eq!(remaining, 0);
        assert!(hash_file(&path, &mut Sha256::new(), &mut remaining).is_err());
        let empty = dir.path().join("empty");
        fs::write(&empty, []).expect("empty file");
        hash_file(&empty, &mut Sha256::new(), &mut remaining).expect("empty file fits zero budget");
    }

    #[test]
    fn fingerprint_detects_dirty_content_changes_without_a_dirty_flag_transition() {
        let dir = tempfile::tempdir().expect("fixture");
        let repo = Repository::init(dir.path()).expect("repo");
        fs::write(dir.path().join("file"), "initial").expect("file");
        let mut index = repo.index().expect("index");
        index.add_path(Path::new("file")).expect("add");
        index.write().expect("write");
        let tree_id = index.write_tree().expect("tree");
        let tree = repo.find_tree(tree_id).expect("tree");
        let signature = git2::Signature::now("Test", "test@example.invalid").expect("signature");
        repo.commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
            .expect("commit");
        fs::write(dir.path().join(".gitignore"), "target/\n").expect("ignore");
        fs::create_dir(dir.path().join("target")).expect("ignored directory");
        fs::write(dir.path().join("target/cache"), "ignored").expect("ignored file");
        let clean = fingerprint(dir.path()).expect("fingerprint with ignored directory");
        fs::write(dir.path().join("target/cache"), "updated ignored cache").expect("ignored update");
        assert_eq!(Some(clean.clone()), fingerprint(dir.path()));
        fs::write(dir.path().join("file"), "first edit").expect("edit");
        let first = fingerprint(dir.path()).expect("dirty fingerprint");
        fs::write(dir.path().join("file"), "second edit").expect("edit");
        let second = fingerprint(dir.path()).expect("dirty fingerprint");
        assert_ne!(clean, first);
        assert_ne!(first, second);
        fs::write(dir.path().join("untracked"), "new").expect("new");
        assert_ne!(Some(second), fingerprint(dir.path()));
    }
}

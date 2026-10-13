use super::*;
use base64::Engine;

const BROWSER: &str = "browser-1";

fn staging(root: &Path) -> Staging {
    let mut staging = Staging::new(root.join(".uploads")).unwrap();
    staging.limits = CloudUploadLimits {
        max_files: 2,
        max_batch_bytes: 8,
        max_chunk_bytes: 4,
    };
    staging
}

fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn chunk(batch: &str, name: &str, whole: &[u8], offset: usize, end: usize) -> CloudStageChunk {
    CloudStageChunk {
        id: BROWSER.into(),
        batch: batch.into(),
        name: name.into(),
        size: whole.len() as u64,
        offset: offset as u64,
        data: base64::engine::general_purpose::STANDARD.encode(&whole[offset..end]),
        sha256: (end == whole.len()).then(|| sha256(whole)),
    }
}

fn files(batch: &str, names: &[&str]) -> CloudStagedFiles {
    CloudStagedFiles {
        batch: batch.into(),
        names: names.iter().map(|name| (*name).to_owned()).collect(),
    }
}

/// Resolves the files and marks their batch used, as an accepted answer does.
fn attach(staging: &mut Staging, batch: &str, names: &[&str], now: Instant) -> Vec<PathBuf> {
    let paths = staging.resolve(BROWSER, &files(batch, names)).unwrap();
    staging.mark_used(BROWSER, batch, now);
    paths
}

/// The refusal of one chunk.
fn refused(staging: &mut Staging, chunk: &CloudStageChunk) -> String {
    staging.stage(chunk, Instant::now()).unwrap_err()
}

/// Stages `whole` in chunks of the test limit.
fn stage_all(staging: &mut Staging, batch: &str, name: &str, whole: &[u8], now: Instant) {
    let mut offset = 0;
    loop {
        let end = (offset + 4).min(whole.len());
        let receipt = staging.stage(&chunk(batch, name, whole, offset, end), now).unwrap();
        assert_eq!(receipt.complete, end == whole.len());
        if receipt.complete {
            return;
        }
        offset = end;
    }
}

#[test]
fn a_file_arrives_in_order_and_is_verified_before_a_browser_gets_it() {
    let root = tempfile::tempdir().unwrap();
    let mut staging = staging(root.path());
    let now = Instant::now();
    let whole = b"abcdefg";
    let first = staging.stage(&chunk("b1", "it's 1.txt", whole, 0, 4), now).unwrap();
    assert_eq!((first.received, first.complete), (4, false));
    assert!(staging.resolve(BROWSER, &files("b1", &["it's 1.txt"])).is_err());
    let last = staging.stage(&chunk("b1", "it's 1.txt", whole, 4, 7), now).unwrap();
    assert!(last.complete);
    let paths = attach(&mut staging, "b1", &["it's 1.txt"], now);
    assert_eq!(paths, [root.path().join(".uploads/browser-1/b1/it's 1.txt")]);
    assert_eq!(std::fs::read(&paths[0]).unwrap(), whole);
    #[cfg(unix)]
    for (path, mode) in [(paths[0].as_path(), 0o600), (paths[0].parent().unwrap(), 0o700)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, mode);
    }
    stage_all(&mut staging, "b2", "empty", b"", now);
    assert!(
        std::fs::read(root.path().join(".uploads/browser-1/b2/empty"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn wrong_bytes_order_or_size_leave_no_file_behind() {
    let root = tempfile::tempdir().unwrap();
    let mut staging = staging(root.path());
    let now = Instant::now();
    let path = root.path().join(".uploads/browser-1/b1/a.txt");
    let whole = b"abcdef";
    assert!(refused(&mut staging, &chunk("fresh", "a.txt", whole, 4, 6)).contains("no start"));
    assert!(staging.batches.is_empty(), "a refused first chunk leaves no batch");

    let mut wrong = chunk("b1", "a.txt", whole, 0, 4);
    wrong.size = 4;
    wrong.sha256 = Some(sha256(b"zzzz"));
    assert!(refused(&mut staging, &wrong).contains("different bytes"));
    assert!(!path.exists());
    assert!(staging.batches.is_empty(), "a batch without files holds no place");

    staging.stage(&chunk("b1", "a.txt", whole, 0, 4), now).unwrap();
    let gap = chunk("b1", "a.txt", whole, 5, 6);
    assert!(refused(&mut staging, &gap).contains("out of order"));
    assert!(!path.exists());

    assert!(refused(&mut staging, &chunk("b1", "a.txt", whole, 4, 6)).contains("no start"));

    let mut early = chunk("b1", "a.txt", whole, 0, 4);
    early.sha256 = Some(sha256(whole));
    assert!(refused(&mut staging, &early).contains("before its last part"));
    assert!(!path.exists());

    let mut long = chunk("b1", "a.txt", whole, 0, 4);
    long.size = 2;
    assert!(refused(&mut staging, &long).contains("more bytes"));
    assert!(staging.resolve(BROWSER, &files("b1", &["a.txt"])).is_err());
}

#[test]
fn names_batches_counts_and_sizes_are_bounded() {
    let root = tempfile::tempdir().unwrap();
    let mut staging = staging(root.path());
    let now = Instant::now();
    for (batch, name) in [("b1", "../escape"), ("b1", "a/b"), ("../b", "a.txt"), ("", "a.txt")] {
        assert!(refused(&mut staging, &chunk(batch, name, b"ab", 0, 2)).contains("invalid"));
    }
    assert!(!root.path().join("escape").exists());

    let mut wide = chunk("b1", "a.txt", b"abcd", 0, 4);
    wide.data = base64::engine::general_purpose::STANDARD.encode(b"abcdefgh");
    assert!(refused(&mut staging, &wide).contains("larger than the worker accepts"));
    // Five or six bytes encode to eight characters, the same length as four bytes.
    let mut padded = chunk("b1", "a.txt", b"abcdef", 0, 4);
    padded.data = base64::engine::general_purpose::STANDARD.encode(b"abcdef");
    assert!(refused(&mut staging, &padded).contains("larger than the worker accepts"));
    assert!(refused(&mut staging, &chunk("b1", "big", &[0; 9], 0, 4)).contains("limit"));

    stage_all(&mut staging, "b1", "one", b"12345", now);
    assert!(refused(&mut staging, &chunk("b1", "two", b"1234", 0, 4)).contains("upload limit"));
    stage_all(&mut staging, "b1", "two", b"123", now);
    assert!(refused(&mut staging, &chunk("b1", "three", b"1", 0, 1)).contains("at most 2"));
    assert!(refused(&mut staging, &chunk("b1", "one", b"12345", 0, 4)).contains("already has all"));

    assert!(staging.resolve(BROWSER, &files("b1", &[])).is_err());
    assert!(staging.resolve(BROWSER, &files("b1", &["one", "one"])).is_err());
    assert!(staging.resolve(BROWSER, &files("b1", &["missing"])).is_err());
    assert!(staging.resolve("browser-2", &files("b1", &["one"])).is_err());
    assert_eq!(attach(&mut staging, "b1", &["two", "one"], now).len(), 2);
    assert!(refused(&mut staging, &chunk("b1", "late", b"1", 0, 1)).contains("already attached"));
}

#[test]
fn used_batches_stay_with_their_browser_and_unused_batches_expire() {
    let root = tempfile::tempdir().unwrap();
    let mut staging = staging(root.path());
    let now = Instant::now();
    stage_all(&mut staging, "used", "a.txt", b"a", now);
    stage_all(&mut staging, "unused", "b.txt", b"b", now);
    attach(&mut staging, "used", &["a.txt"], now);
    let browser = root.path().join(".uploads/browser-1");

    staging.sweep(|_| true, now + Duration::from_mins(9));
    assert!(browser.join("unused/b.txt").exists());
    staging.sweep(|_| true, now + UNUSED_LIFETIME);
    assert!(!browser.join("unused").exists());
    assert!(browser.join("used/a.txt").exists());

    staging.sweep(|id| id != BROWSER, now + UNUSED_LIFETIME);
    assert!(!browser.exists());
    assert!(staging.batches.is_empty());
}

#[test]
fn limits_remove_the_oldest_used_batch_but_never_waiting_files() {
    let root = tempfile::tempdir().unwrap();
    let mut staging = staging(root.path());
    staging.max_staged_bytes = 10;
    staging.max_batches = 3;
    let now = Instant::now();
    for batch in ["old", "newer"] {
        stage_all(&mut staging, batch, "f", b"1234", now);
        attach(&mut staging, batch, &["f"], now);
    }
    stage_all(&mut staging, "waiting", "f", b"12", now);
    assert!(staging.stage(&chunk("fourth", "f", b"1", 0, 1), now).is_ok());
    assert!(!staging.batches.contains_key(&(BROWSER.into(), "old".into())));
    stage_all(&mut staging, "fourth", "g", b"1234567", now);
    assert!(!staging.batches.contains_key(&(BROWSER.into(), "newer".into())));
    assert!(staging.resolve(BROWSER, &files("waiting", &["f"])).is_ok());

    let other = tempfile::tempdir().unwrap();
    let mut waiting = self::staging(other.path());
    waiting.max_staged_bytes = 5;
    waiting.max_batches = 2;
    stage_all(&mut waiting, "a", "f", b"1234", now);
    assert!(refused(&mut waiting, &chunk("b", "f", b"12", 0, 2)).contains("no room"));
    stage_all(&mut waiting, "b", "f", b"1", now);
    assert!(refused(&mut waiting, &chunk("c", "f", b"1", 0, 1)).contains("too many uploads"));
    assert!(waiting.resolve(BROWSER, &files("a", &["f"])).is_ok());
    assert!(waiting.resolve(BROWSER, &files("b", &["f"])).is_ok());
}

#[test]
fn a_new_service_removes_the_files_of_the_previous_one() {
    let root = tempfile::tempdir().unwrap();
    let left = root.path().join(".uploads/browser-9/b1");
    std::fs::create_dir_all(&left).unwrap();
    std::fs::write(left.join("old.txt"), b"old").unwrap();
    let staging = Staging::new(root.path().join(".uploads")).unwrap();
    assert!(!root.path().join(".uploads").exists());
    assert_eq!(staging.limits(), CloudUploadLimits::default());
}

#[cfg(unix)]
#[test]
fn a_planted_link_never_moves_a_staged_file_out_of_its_batch() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mut staging = staging(root.path());
    std::fs::create_dir_all(root.path().join(".uploads")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join(".uploads").join(BROWSER)).unwrap();
    assert!(refused(&mut staging, &chunk("b1", "a.txt", b"ab", 0, 2)).contains("could not save"));
    std::fs::remove_file(root.path().join(".uploads").join(BROWSER)).unwrap();
    std::fs::create_dir_all(root.path().join(".uploads").join(BROWSER)).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join(".uploads").join(BROWSER).join("b2")).unwrap();
    assert!(refused(&mut staging, &chunk("b2", "a.txt", b"ab", 0, 2)).contains("could not save"));
    std::fs::create_dir_all(root.path().join(".uploads").join(BROWSER).join("b3")).unwrap();
    let target = outside.path().join("target.txt");
    std::os::unix::fs::symlink(
        &target,
        root.path().join(".uploads").join(BROWSER).join("b3").join("a.txt"),
    )
    .unwrap();
    assert!(refused(&mut staging, &chunk("b3", "a.txt", b"ab", 0, 2)).contains("more than once"));
    assert!(
        std::fs::read_dir(outside.path()).unwrap().next().is_none(),
        "nothing was written outside"
    );
    assert!(staging.batches.is_empty());
}

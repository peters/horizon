use super::*;
use crate::repository_git::git::Git;
use std::{
    io::Read,
    net::TcpListener,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

fn pointer(bytes: &[u8]) -> Pointer {
    Pointer::parse(
        "asset.bin".into(),
        format!(
            "{PREFIX}oid sha256:{}\nsize {}\n",
            ArtifactDigest::sha256(bytes).as_str(),
            bytes.len()
        )
        .into_bytes(),
    )
    .unwrap()
}

fn repository() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    assert!(
        Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(["init", "--template=", "--initial-branch=main"])
            .current_dir(root.path())
            .output()
            .unwrap()
            .status
            .success()
    );
    root
}

#[test]
fn canonical_pointer_only_and_actual_content_hash_size() {
    let valid = pointer(b"synthetic payload");
    assert!(valid.verify_bytes(b"synthetic payload").is_ok());
    assert!(valid.verify_bytes(b"different payload").is_err());
    let text = String::from_utf8(valid.encoded.clone()).unwrap();
    for bad in [
        text.replace("size 17", "size 017"),
        text.replace("sha256:", "sha1:"),
        text.replace("\nsize", "\next-0-custom value\nsize"),
        text.replace('\n', "\r\n"),
        text.trim_end().into(),
        text.replace("size 17", &format!("size {}", MAX_OBJECT + 1)),
    ] {
        assert!(Pointer::parse("asset.bin".into(), bad.into_bytes()).is_err());
    }
    assert_eq!(pointer(b"").size, 0);
}

#[test]
fn selection_requires_complete_attributes_and_rejects_custom_and_symlink() {
    let root = repository();
    let p = pointer(b"safe");
    fs::write(root.path().join(&p.path), &p.encoded).unwrap();
    let attrs = b"asset.bin\0filter\0lfs\0";
    assert_eq!(plan(root.path(), attrs, b"abc:asset.bin\0", "abc").unwrap().len(), 1);
    for invalid in [
        b"asset.bin\0filter\0custom\0".as_slice(),
        b"asset.bin\0filter\0lfs",
        b"asset.bin\0other\0lfs\0",
    ] {
        assert!(plan(root.path(), invalid, b"", "abc").is_err());
    }
    assert!(plan(root.path(), b"asset.bin\0filter\0unset\0", b"abc:asset.bin\0", "abc").is_err());
    assert!(plan(root.path(), attrs, b"abc:other\0", "abc").is_err());
    fs::remove_file(root.path().join(&p.path)).unwrap();
    std::os::unix::fs::symlink("other", root.path().join(&p.path)).unwrap();
    assert!(plan(root.path(), attrs, b"", "abc").is_err());
}

#[test]
fn count_and_total_admission_precede_any_download() {
    let root = repository();
    let mut attrs = Vec::new();
    let p = pointer(b"x");
    for n in 0..=MAX_PATHS {
        let name = format!("asset{n}");
        fs::write(root.path().join(&name), &p.encoded).unwrap();
        attrs.extend_from_slice(format!("{name}\0filter\0lfs\0").as_bytes());
    }
    assert!(plan(root.path(), &attrs, b"", "abc").is_err());
    let large = String::from_utf8(p.encoded)
        .unwrap()
        .replace("size 1", &format!("size {MAX_OBJECT}"));
    attrs.clear();
    for n in 0..=MAX_TOTAL / MAX_OBJECT {
        let name = format!("asset{n}");
        fs::write(root.path().join(&name), &large).unwrap();
        attrs.extend_from_slice(format!("{name}\0filter\0lfs\0").as_bytes());
    }
    assert!(plan(root.path(), &attrs, b"", "abc").is_err());
}

struct Server {
    endpoint: String,
    stop: Arc<AtomicBool>,
    task: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(pointer: &Pointer, body: Vec<u8>, status: u16) -> Self {
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let address = socket.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let oid = pointer.digest.as_str().to_owned();
        let size = pointer.size;
        let task = thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let Ok((mut client, _)) = socket.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                client.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                client.set_write_timeout(Some(Duration::from_secs(1))).unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
                    if client.read_exact(&mut byte).is_err() {
                        break;
                    }
                    request.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&request);
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|v| v.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if length > 8192 {
                    continue;
                }
                let mut payload = vec![0; length];
                let _ = client.read_exact(&mut payload);
                let response = if request.starts_with(b"POST ") {
                    serde_json::to_vec(&serde_json::json!({"objects":[{"oid":oid,"size":size,
                        "actions":{"download":{"href":format!("http://{address}/object?signed=synthetic_private_value")}}}]})).unwrap()
                } else {
                    body.clone()
                };
                let code = if request.starts_with(b"POST ") { 200 } else { status };
                if code == 504 {
                    thread::sleep(Duration::from_millis(500));
                }
                let header = format!(
                    "HTTP/1.1 {code} Result\r\nContent-Type: application/vnd.git-lfs+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                );
                let _ = client.write_all(header.as_bytes());
                let _ = client.write_all(&response);
            }
        });
        Self {
            endpoint: format!("http://{address}/info/lfs"),
            stop,
            task: Some(task),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.task.take().unwrap().join().unwrap();
    }
}

#[test]
fn installed_smudge_verifies_download_and_blocks_secret_error_logs() {
    for (body, status, success) in [
        (b"synthetic bytes".to_vec(), 200, true),
        (b"wrong bytes".to_vec(), 200, false),
        (b"denied".to_vec(), 403, false),
        (vec![0; 128 * 1024], 200, false),
    ] {
        let root = repository();
        let pointer = pointer(b"synthetic bytes");
        block_logs(root.path()).unwrap();
        let server = Server::new(&pointer, body, status);
        let output = Git::new().smudge_endpoint(root.path(), &pointer, &|| false, &server.endpoint);
        if success {
            assert_eq!(output.unwrap(), b"synthetic bytes");
        } else {
            assert!(output.is_err());
        }
        let logs = root.path().join(".git/lfs/logs");
        assert!(logs.is_file());
        assert_eq!(fs::read(logs).unwrap(), b"");
        for entry in fs::read_dir(root.path().join(".git/lfs/incomplete")).unwrap() {
            assert!(entry.unwrap().metadata().unwrap().len() <= 4096);
        }
        let config = fs::read_to_string(root.path().join(".git/config")).unwrap();
        assert!(!config.contains("signed=") && !config.contains("synthetic_private_value"));
    }
}

#[test]
fn replacement_preserves_mode_and_refuses_dirty_changed_or_wrong_content() {
    use std::os::unix::fs::PermissionsExt;
    let root = repository();
    let pointer = pointer(b"new bytes");
    let path = root.path().join(&pointer.path);
    fs::write(&path, &pointer.encoded).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(replace_pointer(root.path(), &pointer, b"wrong").is_err());
    fs::write(&path, b"user edit").unwrap();
    assert!(replace_pointer(root.path(), &pointer, b"new bytes").is_err());
    assert_eq!(fs::read(&path).unwrap(), b"user edit");
    fs::write(&path, &pointer.encoded).unwrap();
    let mode = fs::metadata(&path).unwrap().mode();
    replace_pointer(root.path(), &pointer, b"new bytes").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new bytes");
    assert_eq!(fs::metadata(path).unwrap().mode(), mode);
}

#[test]
fn expired_or_cancelled_smudge_never_spawns_or_creates_cache() {
    let root = repository();
    let pointer = pointer(b"synthetic");
    let endpoint = "https://must-not-contact.invalid/info/lfs";
    assert_eq!(
        Git::with_timeout(Duration::ZERO).smudge_endpoint(root.path(), &pointer, &|| false, endpoint),
        Err(Error::Interrupted)
    );
    assert_eq!(
        Git::new().smudge_endpoint(root.path(), &pointer, &|| true, endpoint),
        Err(Error::Interrupted)
    );
    assert!(!root.path().join(".git/lfs").exists());
}

#[test]
fn live_child_deadline_preserves_pointer_and_never_creates_completion() {
    let root = repository();
    let pointer = pointer(b"synthetic bytes");
    block_logs(root.path()).unwrap();
    fs::write(root.path().join(&pointer.path), &pointer.encoded).unwrap();
    let server = Server::new(&pointer, b"late error".to_vec(), 504);
    let start = std::time::Instant::now();
    assert_eq!(
        Git::with_timeout(Duration::from_millis(150)).smudge_endpoint(
            root.path(),
            &pointer,
            &|| false,
            &server.endpoint
        ),
        Err(Error::Interrupted)
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(fs::read(root.path().join(&pointer.path)).unwrap(), pointer.encoded);
    assert!(!root.path().join("complete.json").exists());
}

#[test]
fn hydration_verifies_before_and_after_planning_and_version_probe() {
    struct Probe(usize);
    impl Commands for Probe {
        fn run(
            &mut self,
            _directory: &Path,
            args: &[&str],
            _input: &[u8],
            _allow_missing: bool,
            _cancelled: &dyn Fn() -> bool,
        ) -> Result<Vec<u8>, Error> {
            assert_eq!(args, ["lfs", "version"]);
            self.0 += 1;
            Ok(b"git-lfs/3.3.0".to_vec())
        }
    }
    let request = GitPreparation::decode(
        &serde_json::to_vec(&serde_json::json!({"version":1,"workspace_local_id":"lfs-verification",
            "runtime_id":uuid::Uuid::new_v4(),"source":{"repository":"fixture/repository",
            "commit":"a".repeat(40),"branch":"main"},"work_branch":"work/proof"}))
        .unwrap(),
    )
    .unwrap();
    for reject_at in 1..=3 {
        let root = repository();
        let pointer = pointer(b"synthetic");
        fs::write(root.path().join(&pointer.path), &pointer.encoded).unwrap();
        let checks = std::cell::Cell::new(0);
        let mut probe = Probe(0);
        assert_eq!(
            hydrate(
                &mut probe,
                root.path(),
                &request,
                Selection {
                    attributes: b"asset.bin\0filter\0lfs\0",
                    matches: b""
                },
                &mut Budget::default(),
                &|| false,
                &|| {
                    checks.set(checks.get() + 1);
                    if checks.get() == reject_at {
                        Err(Error::UnsafeRoot)
                    } else {
                        Ok(())
                    }
                }
            ),
            Err(Error::UnsafeRoot)
        );
        assert_eq!(probe.0, usize::from(reject_at == 3));
        assert!(!root.path().join(".git/lfs").exists());
        assert_eq!(fs::read(root.path().join(&pointer.path)).unwrap(), pointer.encoded);
    }
    let mut probe = Probe(0);
    assert_eq!(
        hydrate(
            &mut probe,
            Path::new("/nonexistent-lfs-proof"),
            &request,
            Selection {
                attributes: b"",
                matches: b""
            },
            &mut Budget::default(),
            &|| false,
            &|| Err(Error::UnsafeRoot)
        ),
        Err(Error::UnsafeRoot)
    );
    assert_eq!(probe.0, 0);
}

#[test]
fn root_and_children_share_one_lfs_byte_and_path_budget() {
    let mut budget = Budget::default();
    let mut large = pointer(b"x");
    large.size = MAX_OBJECT;
    for _ in 0..MAX_TOTAL / MAX_OBJECT {
        budget.reserve(std::slice::from_ref(&large)).unwrap();
    }
    assert_eq!(budget.reserve(&[pointer(b"x")]), Err(Error::UnsupportedRepository));
    assert_eq!((budget.paths, budget.bytes), (MAX_TOTAL / MAX_OBJECT, MAX_TOTAL));
    let mut budget = Budget::default();
    let zero = pointer(b"");
    for _ in 0..MAX_PATHS {
        budget.reserve(std::slice::from_ref(&zero)).unwrap();
    }
    assert_eq!(budget.reserve(&[zero]), Err(Error::UnsupportedRepository));
    assert_eq!((budget.paths, budget.bytes), (MAX_PATHS, 0));
}

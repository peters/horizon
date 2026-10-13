use super::*;
use horizon_browser_protocol::cloud_view::CloudStageReceipt;
use std::io::Cursor;

/// A control service that keeps every part and answers with a receipt.
#[derive(Default)]
struct Worker {
    chunks: Vec<CloudStageChunk>,
    received: std::collections::BTreeMap<String, Vec<u8>>,
    reply: Option<CloudViewResponse>,
}

impl Link for Worker {
    fn call(&mut self, request: &CloudViewRequest) -> Result<CloudViewResponse, String> {
        let CloudViewRequest::Stage(chunk) = request else {
            panic!("a transfer sends only stage requests");
        };
        self.chunks.push(chunk.clone());
        if let Some(reply) = self.reply.clone() {
            return Ok(reply);
        }
        let bytes = self.received.entry(chunk.name.clone()).or_default();
        bytes.extend(base64::engine::general_purpose::STANDARD.decode(&chunk.data).unwrap());
        let complete = bytes.len() as u64 == chunk.size;
        if complete {
            assert_eq!(chunk.sha256.as_deref(), Some(hex(&Sha256::digest(&*bytes)).as_str()));
        }
        Ok(CloudViewResponse {
            staged: Some(CloudStageReceipt {
                received: bytes.len() as u64,
                complete,
            }),
            ..CloudViewResponse::default()
        })
    }
}

fn limits(max_files: usize, max_batch_bytes: u64) -> CloudUploadLimits {
    CloudUploadLimits {
        max_files,
        max_batch_bytes,
        max_chunk_bytes: MIN_CHUNK_BYTES,
    }
}

fn write(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn files_arrive_in_ordered_parts_with_the_checksum_on_the_last_part() {
    let root = tempfile::tempdir().unwrap();
    let large: Vec<u8> = (0..MIN_CHUNK_BYTES * 2 + 17).map(|byte| (byte % 251) as u8).collect();
    let paths = [
        write(root.path(), "photo.png", &large),
        write(root.path(), "it's ñ 1.txt", b"synthetic text\n"),
        write(root.path(), "empty", b""),
    ];
    let plan = plan(&paths, limits(4, 1 << 20)).unwrap();
    let (mut worker, progress) = (Worker::default(), Progress::default());
    let staged = send(
        &plan,
        "browser-1",
        "b1",
        &mut worker,
        &progress,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(staged.batch, "b1");
    assert_eq!(staged.names, ["photo.png", "it's ñ 1.txt", "empty"]);
    assert_eq!(worker.received["photo.png"], large);
    assert_eq!(worker.received["it's ñ 1.txt"], b"synthetic text\n");
    let photo: Vec<_> = worker.chunks.iter().filter(|chunk| chunk.name == "photo.png").collect();
    assert_eq!(
        photo.iter().map(|chunk| chunk.offset).collect::<Vec<_>>(),
        [0, MIN_CHUNK_BYTES, MIN_CHUNK_BYTES * 2]
    );
    assert!(photo[..2].iter().all(|chunk| chunk.sha256.is_none()));
    let total = large.len() as u64 + 15;
    assert_eq!(progress.sent.load(Ordering::Relaxed), total);
    assert_eq!(progress.total.load(Ordering::Relaxed), total);
}

#[test]
fn plans_bound_counts_sizes_and_folders_and_make_names_valid_on_the_worker() {
    let root = tempfile::tempdir().unwrap();
    let a = write(root.path(), "notes.txt", b"1234");
    std::fs::create_dir(root.path().join("sub")).unwrap();
    let b = write(&root.path().join("sub"), "notes.txt", b"56");
    let c = write(root.path(), "c.txt", b"7");
    assert!(plan(&[], limits(2, 8)).unwrap_err().contains("1 to 2"));
    assert!(plan(&[a.clone(), b.clone(), c.clone()], limits(2, 8)).is_err());
    assert!(
        plan(&[a.clone(), b.clone()], limits(2, 5))
            .unwrap_err()
            .contains("upload limit")
    );
    assert!(
        plan(&[root.path().join("sub")], limits(2, 8))
            .unwrap_err()
            .contains("is a folder")
    );
    assert!(plan(&[root.path().join("missing")], limits(2, 8)).is_err());
    let planned = plan(&[a, b, c], limits(3, 8)).unwrap();
    let names: Vec<_> = planned.files.iter().map(|(_, name, _)| name.as_str()).collect();
    assert_eq!(names, ["notes.txt", "notes (2).txt", "c.txt"]);
    assert_eq!(planned.total, 7);
    assert_eq!(upload_name(Path::new("line\nbreak\u{7f}.txt")), "line_break_.txt");

    let long = format!("{}.txt", "ñ".repeat(200));
    let taken = [(PathBuf::new(), truncate_bytes(&long, MAX_UPLOAD_NAME_BYTES), 0)];
    let name = upload_name(Path::new(&long));
    assert!(valid_upload_name(&name) && name.len() <= MAX_UPLOAD_NAME_BYTES);
    let second = unique_name(&taken, &name);
    assert!(valid_upload_name(&second) && second.ends_with(" (2)"), "{second}");
    assert_eq!(upload_name(Path::new("/")), "file");

    let long_extension = format!("a.{}", "b".repeat(MAX_UPLOAD_NAME_BYTES - 2));
    let taken = [(PathBuf::new(), long_extension.clone(), 0)];
    let second = unique_name(&taken, &long_extension);
    assert!(valid_upload_name(&second) && second != long_extension, "{second}");
    assert!(second.contains(" (2)"));
}

#[test]
fn worker_errors_and_wrong_receipts_end_the_transfer() {
    let root = tempfile::tempdir().unwrap();
    let plan = plan(&[write(root.path(), "a.txt", b"abc")], limits(1, 8)).unwrap();
    let stop = AtomicBool::new(false);
    let run = |reply: CloudViewResponse| {
        let mut worker = Worker {
            reply: Some(reply),
            ..Worker::default()
        };
        send(&plan, "browser-1", "b1", &mut worker, &Progress::default(), &stop).unwrap_err()
    };
    let long = format!("refused\u{1b}[31m {}", "x".repeat(MAX_SHOWN_CHARS * 2));
    let error = run(CloudViewResponse {
        error: Some(long),
        ..CloudViewResponse::default()
    });
    assert!(error.starts_with("refused[31m") && error.ends_with('…'), "{error}");
    assert_eq!(error.chars().count(), MAX_SHOWN_CHARS + 1);
    assert!(run(CloudViewResponse::default()).contains("newer image"));
    let short = CloudViewResponse {
        staged: Some(CloudStageReceipt {
            received: 2,
            complete: true,
        }),
        ..CloudViewResponse::default()
    };
    assert!(run(short).contains("did not confirm"));
    stop.store(true, Ordering::Release);
    let mut worker = Worker::default();
    let stopped = send(&plan, "browser-1", "b1", &mut worker, &Progress::default(), &stop);
    assert_eq!(stopped.unwrap_err(), "The upload stopped");
    assert!(worker.chunks.is_empty());
}

#[test]
fn worker_limits_only_lower_the_host_limits() {
    let host = CloudUploadLimits::default();
    let lowered = effective_limits(limits(2, 10)).unwrap();
    assert_eq!((lowered.max_files, lowered.max_batch_bytes), (2, 10));
    let raised = effective_limits(CloudUploadLimits {
        max_files: usize::MAX,
        max_batch_bytes: u64::MAX,
        max_chunk_bytes: u64::MAX,
    })
    .unwrap();
    assert_eq!(raised, host);
    assert!(effective_limits(limits(0, 10)).is_err());
    assert!(
        effective_limits(CloudUploadLimits {
            max_chunk_bytes: 1,
            ..host
        })
        .is_err()
    );
}

#[test]
fn a_stream_link_reads_one_bounded_reply_per_request() {
    let request = CloudViewRequest::List;
    let mut link = StreamLink {
        input: Vec::new(),
        output: Cursor::new(b"{\"browsers\":[],\"error\":\"busy\"}\n".to_vec()),
    };
    assert_eq!(link.call(&request).unwrap().error.as_deref(), Some("busy"));
    assert_eq!(link.input, b"{\"operation\":\"list\"}\n");
    assert!(link.call(&request).unwrap_err().contains("closed"));
    let mut long = StreamLink {
        input: Vec::new(),
        output: Cursor::new(vec![b' '; usize::try_from(MAX_RESPONSE_BYTES).unwrap() + 10]),
    };
    assert!(long.call(&request).unwrap_err().contains("closed"));
    let mut invalid = StreamLink {
        input: Vec::new(),
        output: Cursor::new(b"not json\n".to_vec()),
    };
    assert!(invalid.call(&request).unwrap_err().contains("invalid"));
}

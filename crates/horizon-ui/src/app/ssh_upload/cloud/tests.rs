use super::*;

#[test]
fn worker_paths_are_pasted_with_posix_quotes_on_every_host() {
    let text = cloud_terminal_text(&[
        "/workspace/.horizon-drops/s/b/screen.png".into(),
        "/workspace/.horizon-drops/s/b/screen shot (2).png".into(),
        "/workspace/.horizon-drops/s/b/O'Brien.txt".into(),
        "/workspace/.horizon-drops/s/b/ærlig 测试.jpeg".into(),
    ]);
    assert_eq!(
        text,
        "/workspace/.horizon-drops/s/b/screen.png \
         '/workspace/.horizon-drops/s/b/screen shot (2).png' \
         '/workspace/.horizon-drops/s/b/O'\"'\"'Brien.txt' \
         '/workspace/.horizon-drops/s/b/ærlig 测试.jpeg' "
    );
}

#[test]
fn a_cloud_flow_starts_uploading_without_reading_the_files() {
    let destination = Destination::new(
        &serde_json::from_value(serde_json::json!({
            "id":"worker1", "name":"fixture", "imageName":"fixture", "desiredStatus":"RUNNING",
            "sshHost":"192.0.2.1", "portMappings":{"22":2222}
        }))
        .unwrap(),
        std::path::Path::new("/synthetic-missing-root"),
        "cloud",
        "session-1",
    )
    .unwrap();
    let flow = SshUploadFlow::cloud(
        ViewportId::ROOT,
        "Claude in Cloud".into(),
        CloudDrop { cloud: 7, destination },
        vec![
            PathBuf::from("/missing/screen shot.png"),
            PathBuf::from("/missing/O'Brien.txt"),
        ],
    );
    assert_eq!(flow.test_summary(), "Uploading");
    let names: Vec<_> = flow.files.iter().map(|file| file.name.as_str()).collect();
    assert_eq!(names, ["screen shot.png", "O'Brien.txt"]);
    assert!(flow.targets_viewport(ViewportId::ROOT));
}

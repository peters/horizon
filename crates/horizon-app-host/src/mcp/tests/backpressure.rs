use super::*;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_mcp_progress_finishes_cancelled_report_without_retrying_the_queued_notifications() {
    let (fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
    let actor = Arc::new(actor);
    let folder = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = folder.path().canonicalize().unwrap();
    let server = NativeMcp::new(actor.clone(), &root, &root).unwrap();
    let mut recipe = String::from("```yaml\ndevice-recipe:\n  version: 1\n  id: backpressure\n  steps:\n");
    for index in 0..64 {
        writeln!(recipe, "    - id: frame-{index}\n      action: screenshot").unwrap();
    }
    recipe.push_str("```\n");
    std::fs::write(fixture.root.path().join("recipe.md"), recipe).unwrap();
    let (server_io, client_io) = tokio::io::duplex(512);
    let serving = tokio::spawn(async move { server.serve(server_io).await.unwrap().waiting().await });
    let (reader, mut writer) = tokio::io::split(client_io);
    let mut reader = tokio::io::BufReader::new(reader);
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":\"native-backpressure-test\",\"version\":\"1\"}}}\n").await.unwrap();
    let mut initialized = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut initialized))
        .await
        .unwrap()
        .unwrap();
    assert!(
        serde_json::from_str::<Value>(&initialized)
            .unwrap()
            .get("result")
            .is_some()
    );
    writer
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"device_test_run\",\"arguments\":{\"lifetime_seconds\":15},\"_meta\":{\"progressToken\":\"blocked\"}}}\n").await.unwrap();
    // Keep the read half open but unread: the real MCP transport blocks while the runner fills its progress queue.
    let report = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            for entry in std::fs::read_dir(folder.path()).unwrap().flatten() {
                let path = entry.path().join("report.json");
                if path.is_file()
                    && let Ok(report) = serde_json::from_slice::<Value>(&std::fs::read(path).unwrap())
                {
                    return report;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("a failed progress consumer must not delay terminal archival for every queued event");
    assert_eq!(report["cancelled"], true, "{report}");
    assert_eq!(report["upload_cleanup_errors"], json!([]));
    assert!(
        report["devices"]
            .as_array()
            .unwrap()
            .iter()
            .all(|device| device["cleanup_confirmed"] == true)
    );
    assert!(
        fixture
            .workspace
            .journal()
            .pending(fixture.workspace.owner())
            .unwrap()
            .is_empty()
    );
    drop((reader, writer));
    tokio::time::timeout(Duration::from_secs(5), serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

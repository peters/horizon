use super::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn host_pull_status_uses_the_real_tool_router_without_a_compute_key() {
    let root = tempfile::tempdir().unwrap();
    let settings = root.path().join("settings.json");
    let value = serde_json::json!({
        "runpod_key_file":root.path().join("absent-compute"),
        "ssh_identity_file":root.path().join("absent-identity"),
        "docker_config":root.path().join("docker"),
        "registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[],
        "registries":{"root":root.path().join("registry"),"bindings":[{
            "repository":"registry.example/team/worker","generation":"synthetic-generation",
            "publish":null,"pull":{"username":"reader","secret_file":root.path().join("absent-pull"),"expires_at":null},
            "read_only_confirmed":true,"retired":[]
        }]}
    });
    std::fs::write(&settings, serde_json::to_vec(&value).unwrap()).unwrap();
    let (client, transport) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(async move {
        Server { settings }
            .serve(transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let (input, mut output) = tokio::io::split(client);
    let mut input = BufReader::new(input);
    for (id, method) in [(1, "server/discover"), (2, "tools/call")] {
        let mut params = serde_json::json!({"_meta":{
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientCapabilities":{}
        }});
        if method == "tools/call" {
            params["name"] = "cloud_registry".into();
            params["arguments"] = serde_json::json!({"operation":"pull_status","repository":"registry.example/team/worker","generation":"synthetic-generation"});
        }
        let request = serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        output.write_all(format!("{request}\n").as_bytes()).await.unwrap();
        let mut line = String::new();
        tokio::time::timeout(std::time::Duration::from_secs(5), input.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(response.get("error").is_none(), "{response}");
        if method == "tools/call" {
            let status = &response["result"]["structuredContent"];
            assert_eq!(status["scope"], "host", "{response}");
            assert_eq!(status["repository"], "registry.example/team/worker");
            assert!(status["validation"].is_null());
            assert!(!line.contains("absent-pull"));
            assert!(!line.contains("absent-compute"));
        }
    }
    drop(output);
    drop(input);
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(!root.path().join("registry/synthetic-generation.json").exists());
}

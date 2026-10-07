use super::*;
use rmcp::ServiceExt as _;
use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt;

#[derive(Clone, Default)]
struct ProgressClient(std::sync::Arc<std::sync::Mutex<Vec<rmcp::model::ProgressNotificationParam>>>);
impl rmcp::ClientHandler for ProgressClient {
    async fn on_progress(
        &self,
        progress: rmcp::model::ProgressNotificationParam,
        _context: rmcp::service::NotificationContext<rmcp::RoleClient>,
    ) {
        self.0.lock().unwrap().push(progress);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn high_level_mcp_run_executes_the_selected_recipe_streams_progress_and_closes_owned_lanes() {
    use rmcp::model::RequestParamsMeta as _;
    let (fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
    let actor = Arc::new(actor);
    let folder = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let server = NativeMcp::new(
        actor.clone(),
        &folder.path().canonicalize().unwrap(),
        &folder.path().canonicalize().unwrap(),
    )
    .unwrap();
    let (server_io, client_io) = tokio::io::duplex(65_536);
    let serving = tokio::spawn(async move { server.serve(server_io).await.unwrap().waiting().await.unwrap() });
    let observations = ProgressClient::default();
    let mut client = observations.clone().serve(client_io).await.unwrap();
    let mut request = CallToolRequestParams::new("device_test_run")
        .with_arguments(json!({"lifetime_seconds":15}).as_object().unwrap().clone());
    request.set_meta(rmcp::model::RequestMetaObject::with_progress_token(
        rmcp::model::ProgressToken(rmcp::model::NumberOrString::String("native-run".into())),
    ));
    let result = client.call_tool(request).await.unwrap();
    assert_ne!(result.is_error, Some(true));
    let report = result.structured_content.unwrap();
    assert_eq!(report["builds"].as_array().unwrap().len(), 1);
    assert_eq!(report["devices"].as_array().unwrap().len(), 1);
    assert_eq!(report["devices"][0]["steps"][0]["passed"], true, "{report}");
    assert_eq!(report["devices"][0]["cleanup_confirmed"], true);
    assert_eq!(report["upload_cleanup_errors"], json!([]));
    let screenshot = report["devices"][0]["steps"][0]["screenshot"]["path"].as_str().unwrap();
    assert!(std::path::Path::new(screenshot).is_file());
    {
        // The client library schedules notification handlers concurrently; compare sequence values independently of handler completion order.
        let mut events = observations.0.lock().unwrap().clone();
        events.sort_by(|a, b| a.progress.total_cmp(&b.progress));
        assert!(events.iter().any(|event| {
            event
                .message
                .as_ref()
                .is_some_and(|message| message.contains("\"phase\":\"build\""))
        }));
        assert!(events.windows(2).all(|pair| pair[0].progress < pair[1].progress));
    }
    assert!(
        fixture
            .workspace
            .journal()
            .pending(fixture.workspace.owner())
            .unwrap()
            .is_empty()
    );
    let mut recipe = String::from("```yaml\ndevice-recipe:\n  version: 1\n  id: captures\n  steps:\n");
    for index in 0..35 {
        writeln!(recipe, "    - id: frame-{index}\n      action: screenshot").unwrap();
    }
    recipe.push_str("```\n");
    std::fs::write(fixture.root.path().join("recipe.md"), recipe).unwrap();
    let many = client
        .call_tool(
            CallToolRequestParams::new("device_test_run")
                .with_arguments(json!({"lifetime_seconds":15}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_ne!(many.is_error, Some(true));
    let many = many.structured_content.unwrap();
    let steps = many["devices"][0]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 35);
    let mut expired = 0;
    for step in steps {
        assert_eq!(step["passed"], true);
        let capture = &step["screenshot"];
        if capture["state"] == "expired" {
            assert!(capture["path"].is_null());
            expired += 1;
        } else {
            assert_eq!(capture["state"], "available");
            assert!(std::path::Path::new(capture["path"].as_str().unwrap()).is_file());
        }
    }
    assert_eq!(expired, 0);
    client.close().await.unwrap();
    serving.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typed_mcp_transport_controls_owned_native_sessions_and_rejects_foreign_or_extra_inputs() {
    let (_fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
    let actor = Arc::new(actor);
    let folder = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = folder.path().canonicalize().unwrap();
    let server = NativeMcp::new(actor.clone(), &root, &root).unwrap();
    let (server_io, client_io) = tokio::io::duplex(65_536);
    let serving = tokio::spawn(async move { server.serve(server_io).await.unwrap().waiting().await.unwrap() });
    let mut client = ().serve(client_io).await.unwrap();
    let tools = client.list_tools(None).await.unwrap();
    assert_eq!(tools.tools.len(), 13);
    for tool in tools.tools {
        assert_eq!(tool.input_schema.get("additionalProperties"), Some(&json!(false)));
        let schema = serde_json::to_string(&tool.input_schema).unwrap();
        for forbidden in ["authorization", "password", "endpoint", "root", "argv"] {
            assert!(!schema.contains(forbidden));
        }
    }
    let app = client
        .call_tool(
            CallToolRequestParams::new("app_upload").with_arguments(
                json!({"platform":"ios","lifetime_seconds":30})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_ne!(app.is_error, Some(true));
    let artifact_id = app.structured_content.unwrap()["id"].as_str().unwrap().to_owned();
    let create = client
        .call_tool(
            CallToolRequestParams::new("app_session_create").with_arguments(
                json!({"artifact":artifact_id,"matrix_index":0,"lifetime_seconds":10})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_ne!(create.is_error, Some(true));
    let session = create.structured_content.unwrap()["id"].as_str().unwrap().to_owned();
    let snapshot = client
        .call_tool(
            CallToolRequestParams::new("app_snapshot")
                .with_arguments(json!({"session":session}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_ne!(snapshot.is_error, Some(true));
    assert!(serde_json::to_string(&snapshot).unwrap().contains("menu.open"));
    let foreign = client
        .call_tool(
            CallToolRequestParams::new("app_session_close").with_arguments(
                json!({"session":Uuid::new_v4().to_string()})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(foreign.is_error, Some(true));
    bounded_wait_inputs(&client, &session).await;
    assert_private_audit_cursor(&client).await;
    actor.snapshot(handle(&session).unwrap()).unwrap();
    let invalid = client
        .call_tool(
            CallToolRequestParams::new("app_session_close").with_arguments(
                json!({"session":session,"endpoint":"synthetic-untrusted-endpoint"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await;
    assert!(invalid.is_err() || invalid.is_ok_and(|value| value.is_error == Some(true)));
    actor.snapshot(handle(&session).unwrap()).unwrap();
    screenshot_action_requires_capture(&client, &actor, &session).await;
    let session = reset_session(&client, &actor, &session).await;
    owned_media_survives_close_and_lane_pruning(&client, &actor, &session, &artifact_id).await;
    let close = client
        .call_tool(
            CallToolRequestParams::new("app_session_close")
                .with_arguments(json!({"session":session}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_ne!(close.is_error, Some(true));
    client.close().await.unwrap();
    serving.await.unwrap();
    actor.release_upload(handle(&artifact_id).unwrap()).unwrap();
}

async fn screenshot_action_requires_capture(client: &rmcp::Peer<rmcp::RoleClient>, actor: &Actor, session: &str) {
    let capture_action = client
        .call_tool(
            CallToolRequestParams::new("app_act").with_arguments(
                json!({"session":session,"action":{"action":"screenshot"}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(capture_action.is_error, Some(true));
    assert!(
        serde_json::to_string(&capture_action)
            .unwrap()
            .contains("app_screenshot_requires_capture")
    );
    actor.snapshot(handle(session).unwrap()).unwrap();
    let captured = client
        .call_tool(
            CallToolRequestParams::new("app_screenshot")
                .with_arguments(json!({"session":session}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_ne!(captured.is_error, Some(true));
    assert!(
        captured
            .content
            .iter()
            .any(|content| serde_json::to_value(content).unwrap()["type"] == "image")
    );
}

async fn reset_session(client: &rmcp::Peer<rmcp::RoleClient>, actor: &Actor, session: &str) -> String {
    let reset = client
        .call_tool(
            CallToolRequestParams::new("app_act").with_arguments(
                json!({"session":session,"action":{"action":"reset"}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_ne!(reset.is_error, Some(true));
    let replacement = reset.structured_content.unwrap()["replacement"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(replacement, session);
    assert!(actor.snapshot(handle(session).unwrap()).is_err());
    actor.snapshot(handle(&replacement).unwrap()).unwrap();
    replacement
}

async fn owned_media_survives_close_and_lane_pruning(
    client: &rmcp::Peer<rmcp::RoleClient>,
    actor: &Actor,
    session: &str,
    artifact_id: &str,
) {
    let stopped = client
        .call_tool(
            CallToolRequestParams::new("app_video").with_arguments(
                json!({"session":session,"operation":"stop"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_ne!(stopped.is_error, Some(true));
    let video = stopped.structured_content.unwrap();
    assert!(std::path::Path::new(video["path"].as_str().unwrap()).is_file());
    assert!(actor.snapshot(handle(session).unwrap()).is_err());
    let replacement = client
        .call_tool(
            CallToolRequestParams::new("app_session_create").with_arguments(
                json!({"artifact":artifact_id,"matrix_index":0,"lifetime_seconds":10})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let video = client
        .call_tool(
            CallToolRequestParams::new("app_video").with_arguments(
                json!({"session":session,"operation":"get"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_ne!(video.is_error, Some(true));
    for kind in ["device", "crash", "appium", "network"] {
        let logs = client
            .call_tool(
                CallToolRequestParams::new("app_logs")
                    .with_arguments(json!({"session":session,"kind":kind}).as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_ne!(logs.is_error, Some(true));
        let response = serde_json::to_string(&logs).unwrap();
        assert!(!response.contains("synthetic_session_"));
        assert!(!response.contains("fixture stack frame"));
    }
    client
        .call_tool(
            CallToolRequestParams::new("app_session_close")
                .with_arguments(json!({"session":replacement}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
}

async fn assert_private_audit_cursor(client: &rmcp::Peer<rmcp::RoleClient>) {
    let audit = client
        .call_tool(
            CallToolRequestParams::new("app_audit")
                .with_arguments(json!({"after_sequence":0,"limit":256}).as_object().unwrap().clone()),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let events = audit["entries"].as_array().unwrap();
    assert!(
        events
            .iter()
            .any(|event| event["status"] == "completed" && event["action"] == "snapshot")
    );
    let failure = events.iter().find(|event| event["status"] == "failed").unwrap();
    assert_eq!(failure["error_code"], "app_owner_refused");
    assert!(
        events
            .iter()
            .any(|event| event["operation"] == failure["operation"] && event["status"] == "accepted")
    );
    let encoded = serde_json::to_string(&audit).unwrap();
    for forbidden in [
        "menu.open",
        "bs://",
        "synthetic-untrusted-endpoint",
        "target",
        "text",
        "password",
    ] {
        assert!(!encoded.contains(forbidden));
    }
    let cursor = audit["next_sequence"].as_u64().unwrap();
    let page = client
        .call_tool(
            CallToolRequestParams::new("app_audit")
                .with_arguments(json!({"after_sequence":cursor,"limit":1}).as_object().unwrap().clone()),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(page["stream"], audit["stream"]);
    assert_eq!(page["next_sequence"], cursor);
    assert!(page["entries"].as_array().unwrap().is_empty());
    for input in [
        json!({"after_sequence":cursor+1,"limit":1}),
        json!({"after_sequence":0,"limit":0}),
    ] {
        let invalid = client
            .call_tool(CallToolRequestParams::new("app_audit").with_arguments(input.as_object().unwrap().clone()))
            .await
            .unwrap();
        assert_eq!(invalid.is_error, Some(true));
    }
}

async fn bounded_wait_inputs(
    client: &rmcp::service::RunningService<rmcp::RoleClient, impl rmcp::ClientHandler>,
    session: &str,
) {
    for target in [
        json!({"by":"identifier","value":"x".repeat(513)}),
        json!({"by":"label","value":"invalid\nlabel"}),
        json!({"by":"ref","value":"invalid ref"}),
    ] {
        let response = client
            .call_tool(
                CallToolRequestParams::new("app_wait").with_arguments(
                    json!({"session":session,"target":target,"state":"visible","timeout_millis":10})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(response.is_error, Some(true));
        assert!(
            serde_json::to_string(&response)
                .unwrap()
                .contains("device_recipe_invalid")
        );
    }
}

mod backpressure;

use super::*;

#[test]
fn unsupported_remote_handoffs_leave_the_entire_manifest_unchanged() {
    let home = tempfile::tempdir().expect("isolated home");
    seed_home(home.path());
    let mut agent = McpProcess::start(home.path(), AGENT_A, Some(HOST_A));
    for hidden in [false, true] {
        for resume in [false, true] {
            let mut panel = read_manifest(home.path(), SAME_WORKSPACE_PANEL);
            panel.remote_target = Some("synthetic-phone".into());
            panel.hidden = hidden;
            panel.owner = resume.then(|| ManifestOwner {
                name: AGENT_A.into(),
                tty: None,
                updated_at: horizon_browser_control::manifest::now_millis(),
            });
            panel.handoff = resume.then(|| ManifestHandoff {
                request_id: "existing-request".into(),
                reason: "synthetic handoff".into(),
                requested_at: horizon_browser_control::manifest::now_millis(),
                done: false,
            });
            write_manifest(home.path(), &panel);
            let before = serde_json::to_value(read_manifest(home.path(), SAME_WORKSPACE_PANEL)).expect("before");
            let mut arguments = json!({
                "panel_id": SAME_WORKSPACE_PANEL,
                "reason": "synthetic request",
                "wait": true,
                "timeout_millis": 1000
            });
            if resume {
                arguments["resume_request_id"] = json!("existing-request");
            }
            let result = agent.call("browser_handoff", &arguments);
            assert_eq!(result["isError"], true, "{result}");
            assert!(
                result["content"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("unsupported_backend")
            );
            let after = serde_json::to_value(read_manifest(home.path(), SAME_WORKSPACE_PANEL)).expect("after");
            assert_eq!(before, after, "hidden={hidden}, resume={resume}");
        }
    }
    agent.close();
}

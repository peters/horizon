//! The card contract over one stream, and the refusal of every other operation.

use std::time::Duration;

use horizon_browser_control::manifest::block::{self, Operation};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

use super::serve_stream;
use crate::HorizonBrowserMcp;
use crate::controller::BrowserController;

const ACTOR: &str = "horizon:machine/build-1/fix-71";
const HOST: &str = "0123456789abcdef0123456789abcdef";
const HANDSHAKE: Duration = Duration::from_secs(20);

fn every_card_operation() -> Vec<Operation> {
    vec![
        Operation::Create {
            title: None,
            document: serde_json::json!({"components": []}),
        },
        Operation::Update {
            panel_id: "panel".to_owned(),
            document: serde_json::json!({"components": []}),
        },
        Operation::List,
        Operation::Inspect {
            panel_id: "panel".to_owned(),
        },
        Operation::NextClick {
            panel_id: None,
            timeout_millis: None,
        },
        Operation::Visibility {
            panel_id: "panel".to_owned(),
            visible: false,
        },
        Operation::Close {
            panel_id: "panel".to_owned(),
        },
    ]
}

#[test]
fn a_card_socket_lists_the_card_tool_alone() {
    let server = HorizonBrowserMcp::for_cards(ACTOR, HOST);
    assert_eq!(server.tool_names(), vec!["block_panel".to_owned()]);
    let full = HorizonBrowserMcp::for_host(ACTOR, HOST);
    assert!(full.tool_names().len() > 20, "a local server lists every tool");
}

#[test]
fn the_seven_card_operations_pass_the_guard_and_no_other_does() {
    for operation in every_card_operation() {
        assert!(operation.is_card(), "{operation:?}");
    }
    for operation in [
        Operation::Machines,
        Operation::SecretNames,
        Operation::Presets,
        Operation::ServerStatus,
        Operation::BrowserHostStatus,
        Operation::MachineDelete {
            name: "build-1".to_owned(),
        },
        Operation::DeleteMachineKey {
            name: "build-1".to_owned(),
        },
    ] {
        assert!(!operation.is_card(), "{operation:?}");
    }
}

#[tokio::test]
async fn a_card_socket_refuses_an_operation_outside_the_cards() {
    let controller = BrowserController::for_host(ACTOR, HOST).only_cards();
    let outcome = controller
        .block_panel(Operation::MachineDelete {
            name: "build-1".to_owned(),
        })
        .await
        .expect("the guard answers before the queue");
    assert!(matches!(
        outcome,
        horizon_browser_control::manifest::block::Outcome::Failed { code, .. } if code == block::CARDS_ONLY
    ));
}

type Client = tokio::io::DuplexStream;

async fn ask(write: &mut tokio::io::WriteHalf<Client>, body: serde_json::Value) {
    let line = format!("{body}\n");
    write.write_all(line.as_bytes()).await.expect("write a request");
    write.flush().await.expect("flush");
}

async fn answer(lines: &mut tokio::io::Lines<BufReader<tokio::io::ReadHalf<Client>>>, id: i64) -> serde_json::Value {
    loop {
        let line = tokio::time::timeout(HANDSHAKE, lines.next_line())
            .await
            .expect("an answer in time")
            .expect("a read")
            .expect("a line");
        let value: serde_json::Value = serde_json::from_str(&line).expect("JSON-RPC");
        if value["id"] == id {
            return value;
        }
    }
}

#[tokio::test]
async fn one_stream_carries_initialize_and_the_one_tool() {
    let (client, far_end) = tokio::io::duplex(64 * 1024);
    let served = tokio::spawn(async move { serve_stream(ACTOR, HOST, far_end).await.is_ok() });
    let (read, mut write) = tokio::io::split(client);
    let mut lines = BufReader::new(read).lines();
    ask(
        &mut write,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "card-test", "version": "1"}
            }
        }),
    )
    .await;
    let hello = answer(&mut lines, 1).await;
    assert!(hello["result"]["serverInfo"]["name"].is_string(), "{hello}");
    ask(
        &mut write,
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    ask(
        &mut write,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    )
    .await;
    let tools = answer(&mut lines, 2).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(names, vec!["block_panel"]);
    drop(write);
    drop(lines);
    let _ = tokio::time::timeout(HANDSHAKE, served).await;
}

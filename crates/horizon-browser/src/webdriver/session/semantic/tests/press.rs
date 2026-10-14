use serde_json::{Value, json};

use crate::webdriver::session::viewport::tests::{bidi_fixture, events, fixture_driver};
use crate::webdriver::test_server::{Reply, Server};
use crate::{AgentAction, BrowserControlAction, BrowserControlValue, BrowserKey, BrowserModifiers};

fn shift_tab(count: u32) -> AgentAction {
    AgentAction {
        action_id: "press".into(),
        actor: "agent".into(),
        requested_at_millis: crate::navigation::now_millis(),
        action: BrowserControlAction::Press {
            key: BrowserKey::Tab,
            modifiers: BrowserModifiers {
                shift: true,
                ..BrowserModifiers::none()
            },
            count,
        },
    }
}

fn key_actions(payload: &Value) -> Vec<(String, String)> {
    assert_eq!(payload["actions"].as_array().map(Vec::len), Some(1), "one key source");
    payload["actions"][0]["actions"]
        .as_array()
        .expect("key actions")
        .iter()
        .map(|action| {
            (
                action["type"].as_str().expect("type").to_string(),
                action["value"].as_str().expect("value").to_string(),
            )
        })
        .collect()
}

#[test]
fn remote_press_sends_every_chord_in_one_sequence_and_releases_its_modifiers() {
    // The fixture driver is a remote session, so presses travel as classic
    // Perform Actions; local Firefox sends the same payload over BiDi.
    let classic = Server::start((0..4).map(|_| Reply::json(200, &json!({"value": null}))).collect());
    let (link, worker) = bidi_fixture(false, true);
    let mut driver = fixture_driver(&classic, link);
    let result = driver.execute_agent_action(&shift_tab(2), &events());
    assert_eq!(result, Ok(BrowserControlValue::Accepted));
    drop(driver);
    worker.join().expect("fixture");
    let performed = classic
        .recorded()
        .into_iter()
        .filter(|request| request.path == "/session/test/actions")
        .map(|request| {
            assert_eq!(request.method, "POST");
            serde_json::from_str::<Value>(&request.body).expect("actions payload")
        })
        .collect::<Vec<_>>();
    let (shift, tab) = ("\u{e008}".to_string(), "\u{e004}".to_string());
    let press = vec![
        ("keyDown".to_string(), shift.clone()),
        ("keyDown".to_string(), tab.clone()),
        ("keyUp".to_string(), tab),
        ("keyUp".to_string(), shift),
    ];
    assert_eq!(
        performed.iter().map(key_actions).collect::<Vec<_>>(),
        vec![[press.clone(), press].concat()]
    );
}

#[test]
fn a_refused_press_releases_the_input_state() {
    let classic = Server::start(vec![
        Reply::json(500, &json!({"value": {"error": "unknown error", "message": "refused"}})),
        Reply::json(200, &json!({"value": null})),
    ]);
    let (link, worker) = bidi_fixture(false, true);
    let mut driver = fixture_driver(&classic, link);
    let error = driver
        .execute_agent_action(&shift_tab(1), &events())
        .expect_err("refused press");
    assert_eq!(error.code, "input_failed");
    drop(driver);
    worker.join().expect("fixture");
    let requests = classic
        .recorded()
        .into_iter()
        .map(|request| (request.method, request.path))
        .collect::<Vec<_>>();
    assert_eq!(
        requests,
        [
            ("POST".to_string(), "/session/test/actions".to_string()),
            ("DELETE".to_string(), "/session/test/actions".to_string()),
        ]
    );
}

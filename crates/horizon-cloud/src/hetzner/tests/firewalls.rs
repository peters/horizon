mod management;

use super::*;
use crate::hetzner::firewalls::{Direction, Protocol, Rule, Scope};

fn scope() -> Scope {
    Scope::new("exposure-1", OPERATION, 42).unwrap()
}
fn rule() -> Rule {
    Rule {
        description: None,
        direction: Direction::In,
        protocol: Protocol::Tcp,
        port: Some("20000".into()),
        source_ips: vec!["192.0.2.0/24".into()],
        destination_ips: Vec::new(),
    }
}
fn firewall(target: bool) -> Value {
    json!({"id":7,"name":"horizon-cloud-exposure-1","labels":{"horizon-operation":"exposure-1","horizon-exposure-server":"42","horizon-exposure-allocation":OPERATION},"rules":[rule()],"applied_to":if target {json!([{"type":"server","server":{"id":42}}])} else {json!([])}})
}
fn server() -> Value {
    json!({"server":{"id":42,"name":"horizon-cloud-op-1","status":"running","labels":{"horizon-operation":OPERATION},"public_net":{"ipv4":{"ip":"192.0.2.42"}},"server_type":{"name":"cpx22","cores":2,"memory":4.0,"disk":40},"location":{"name":"fsn1"}}})
}

#[test]
fn create_is_unattached_and_an_uncertain_result_never_repeats_the_post() {
    let (h, requests, task) = provider(vec![
        (200, server()),
        (201, json!({"firewall":firewall(false),"actions":[]})),
        (200, server()),
        (500, error("server_error", "uncertain")),
    ]);
    let created = h
        .create_exposure_firewall_once(&scope(), &[rule()], &Cancellation::default())
        .unwrap();
    assert_eq!(created.firewall.id, 7);
    assert!(created.firewall.applied_to.is_empty());
    assert!(
        h.create_exposure_firewall_once(&scope(), &[rule()], &Cancellation::default())
            .is_err()
    );
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    let body = request_body(&requests[1]);
    assert!(body.get("apply_to").is_none());
    assert_eq!(body["labels"]["horizon-exposure-server"], "42");
    assert!(requests[3].starts_with("POST /firewalls "));
}

#[test]
fn apply_remove_and_delete_use_only_the_original_numeric_id_and_exact_server() {
    let (h, requests, task) = provider(vec![
        (200, server()),
        (200, json!({"firewall":firewall(false)})),
        (201, json!({"actions":[action(11,"running")]})),
        (200, json!({"action":action(11,"success")})),
        (200, json!({"firewall":firewall(true)})),
        (201, json!({"actions":[action(12,"success")]})),
        (200, json!({"firewall":firewall(false)})),
        (204, Value::Null),
        (404, error("not_found", "absent")),
    ]);
    let cancel = Cancellation::default();
    let actions = h.apply_exposure_firewall(&scope(), 7, &cancel).unwrap();
    h.wait_firewall_actions(&actions, &cancel).unwrap();
    h.remove_exposure_firewall(&scope(), 7, &cancel).unwrap();
    h.delete_exposure_firewall(&scope(), 7, &cancel).unwrap();
    h.delete_exposure_firewall(&scope(), 7, &cancel).unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[2].starts_with("POST /firewalls/7/actions/apply_to_resources "));
    assert_eq!(
        request_body(&requests[2]),
        json!({"apply_to":[{"type":"server","server":{"id":42}}]})
    );
    assert_eq!(
        request_body(&requests[5]),
        json!({"remove_from":[{"type":"server","server":{"id":42}}]})
    );
    assert!(requests[7].starts_with("DELETE /firewalls/7 "));
}

#[test]
fn foreign_target_label_selector_and_attached_delete_are_refused_without_mutation() {
    let mut wrong = firewall(true);
    wrong["applied_to"][0]["server"]["id"] = 43.into();
    let mut selected = firewall(false);
    selected["applied_to"] = json!([{"type":"label_selector","label_selector":{"selector":"all"}}]);
    let mut extra = firewall(true);
    extra["labels"]["foreign"] = "yes".into();
    let (h, requests, task) = provider(vec![
        (200, server()),
        (200, json!({"firewall":wrong})),
        (200, json!({"firewall":selected})),
        (200, json!({"firewall":extra})),
        (200, json!({"firewall":firewall(true)})),
    ]);
    let c = Cancellation::default();
    assert!(h.apply_exposure_firewall(&scope(), 7, &c).is_err());
    assert!(h.remove_exposure_firewall(&scope(), 7, &c).is_err());
    assert!(h.delete_exposure_firewall(&scope(), 7, &c).is_err());
    assert!(h.delete_exposure_firewall(&scope(), 7, &c).is_err());
    task.join().unwrap();
    assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
}

#[test]
fn empty_resource_expansion_is_accepted_but_any_additional_resource_refuses() {
    let mut empty = firewall(true);
    empty["applied_to"][0]["applied_to_resources"] = json!([]);
    let mut expanded = empty.clone();
    expanded["applied_to"][0]["applied_to_resources"] = json!([{"type":"server","server":{"id":43}}]);
    let (h, requests, task) = provider(vec![
        (200, json!({"firewall":empty})),
        (200, json!({"firewall":expanded})),
    ]);
    let cancel = Cancellation::default();
    assert!(h.inspect_exposure_firewall(&scope(), 7, &cancel).unwrap().is_some());
    assert!(h.remove_exposure_firewall(&scope(), 7, &cancel).is_err());
    task.join().unwrap();
    assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
}

#[test]
fn changed_allocation_cannot_create_or_apply_and_rule_update_reads_current_ownership() {
    let mut foreign = server();
    foreign["server"]["labels"]["horizon-operation"] = "other".into();
    let mut changed = firewall(false);
    changed["id"] = 8.into();
    let (h, requests, task) = provider(vec![
        (200, foreign),
        (200, server()),
        (200, json!({"firewall":changed})),
    ]);
    let c = Cancellation::default();
    assert!(h.create_exposure_firewall_once(&scope(), &[rule()], &c).is_err());
    assert!(h.set_exposure_firewall_rules(&scope(), 7, &[rule()], &c).is_err());
    task.join().unwrap();
    assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
}

#[test]
fn rule_replacement_sends_the_complete_set_and_failed_actions_remain_errors() {
    let (h, requests, task) = provider(vec![
        (200, server()),
        (200, json!({"firewall":firewall(true)})),
        (201, json!({"actions":[action(19,"running")]})),
        (
            200,
            json!({"action":{"id":19,"status":"error","error":{"message":"in use"}}}),
        ),
    ]);
    let rules = [rule()];
    let cancel = Cancellation::default();
    let actions = h.set_exposure_firewall_rules(&scope(), 7, &rules, &cancel).unwrap();
    assert!(h.wait_firewall_actions(&actions, &cancel).is_err());
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[2].starts_with("POST /firewalls/7/actions/set_rules "));
    assert_eq!(request_body(&requests[2]), json!({"rules":rules}));
    assert!(requests[3].starts_with("GET /actions/19 "));
}

#[test]
fn reconciliation_follows_pages_and_refuses_multiple_or_foreign_candidates() {
    let mut foreign = firewall(false);
    foreign["labels"]["horizon-exposure-allocation"] = "other".into();
    let (h, requests, task) = provider(vec![
        (200, json!({"firewalls":[],"meta":{"pagination":{"next_page":2}}})),
        (200, listing("firewalls", json!([firewall(false)]))),
        (200, listing("firewalls", json!([firewall(false), firewall(false)]))),
        (200, listing("firewalls", json!([foreign]))),
    ]);
    let c = Cancellation::default();
    assert_eq!(h.find_exposure_firewall(&scope(), &c).unwrap().unwrap().id, 7);
    assert!(h.find_exposure_firewall(&scope(), &c).is_err());
    assert!(h.find_exposure_firewall(&scope(), &c).is_err());
    task.join().unwrap();
    assert!(
        requests.lock().unwrap()[1]
            .starts_with("GET /firewalls?label_selector=horizon-operation%3Dexposure-1&page=2&per_page=50 ")
    );
}

#[test]
fn invalid_policy_fields_and_unknown_rule_fields_never_become_a_broader_request() {
    assert!(Scope::new("../other", OPERATION, 42).is_err());
    assert!(Scope::new("exposure-1", OPERATION, 0).is_err());
    let (h, requests, task) = provider(vec![]);
    for text in ["0", "65536", "20001-20000", "22,20000"] {
        let mut invalid = rule();
        invalid.port = Some(text.into());
        assert!(
            h.create_exposure_firewall_once(&scope(), &[invalid], &Cancellation::default())
                .is_err()
        );
    }
    for text in ["0.0.0.0/33", "::/129", "192.0.2.0", "bad/0"] {
        let mut invalid = rule();
        invalid.source_ips = vec![text.into()];
        assert!(
            h.create_exposure_firewall_once(&scope(), &[invalid], &Cancellation::default())
                .is_err()
        );
    }
    let mut unknown = serde_json::to_value(rule()).unwrap();
    unknown["new_filter"] = true.into();
    assert!(serde_json::from_value::<Rule>(unknown).is_err());
    assert!(
        h.wait_firewall_actions(
            &[
                serde_json::from_value(action(1, "success")).unwrap(),
                serde_json::from_value(action(1, "success")).unwrap()
            ],
            &Cancellation::default()
        )
        .is_err()
    );
    task.join().unwrap();
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn effective_rule_prefixes_combine_across_ports_but_count_each_direction() {
    let repeated: Vec<_> = (0..50)
        .map(|index| Rule {
            port: Some((20000 + index).to_string()),
            source_ips: (0..100).map(|prefix| format!("198.51.{prefix}.0/24")).collect(),
            ..rule()
        })
        .collect();
    let (h, requests, task) = provider(vec![
        (200, server()),
        (201, json!({"firewall":firewall(false),"actions":[]})),
    ]);
    h.create_exposure_firewall_once(&scope(), &repeated, &Cancellation::default())
        .unwrap();
    let mut overflow: Vec<_> = (0..5)
        .map(|group| Rule {
            source_ips: (0..100).map(|prefix| format!("198.{group}.{prefix}.0/24")).collect(),
            ..rule()
        })
        .collect();
    overflow.push(Rule {
        direction: Direction::Out,
        source_ips: vec![],
        destination_ips: vec![overflow[0].source_ips[0].clone()],
        ..rule()
    });
    assert!(matches!(
        h.create_exposure_firewall_once(&scope(), &overflow, &Cancellation::default()),
        Err(CloudError::Invalid(_))
    ));
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "501 directional prefixes refuse before I/O");
    assert_eq!(request_body(&requests[1])["rules"], json!(repeated));
}

#[test]
fn equivalent_ipv6_spellings_share_the_same_effective_network_prefix() {
    let repeated: Vec<_> = (0..6)
        .map(|spelling| Rule {
            port: Some((20000 + spelling).to_string()),
            source_ips: (0..100)
                .map(|prefix| match spelling {
                    0 => format!("2001:db8:{prefix:x}::/64"),
                    1 => format!("2001:0db8:{prefix:x}::/64"),
                    2 => format!("2001:db8:{prefix:04x}::/64"),
                    3 => format!("2001:db8:{prefix:x}:0::/64"),
                    4 => format!("2001:db8:{prefix:x}:0:0:0:0:0/64"),
                    _ => format!("2001:0DB8:{prefix:04X}:0000:0000:0000:0000:0000/64"),
                })
                .collect(),
            ..rule()
        })
        .collect();
    let (h, requests, task) = provider(vec![
        (200, server()),
        (201, json!({"firewall":firewall(false),"actions":[]})),
    ]);
    h.create_exposure_firewall_once(&scope(), &repeated, &Cancellation::default())
        .unwrap();
    let distinct: Vec<_> = (0..6)
        .map(|group| Rule {
            source_ips: (0..100)
                .map(|prefix| format!("2001:db8:{:x}::/64", group * 100 + prefix))
                .collect(),
            ..rule()
        })
        .collect();
    assert!(matches!(
        h.create_exposure_firewall_once(&scope(), &distinct, &Cancellation::default()),
        Err(CloudError::Invalid(_))
    ));
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "600 distinct prefixes refuse before I/O");
    assert_eq!(request_body(&requests[1])["rules"], json!(repeated));
}

use super::*;
use crate::hetzner::firewalls::{
    Firewall,
    management::{Journal, ManagementRules, Update},
};

fn management() -> ManagementRules {
    ManagementRules::new(vec![tcp(22, "192.0.2.10/32")]).unwrap()
}
fn tcp(port: u16, source: &str) -> Rule {
    Rule {
        port: Some(port.to_string()),
        source_ips: vec![source.into()],
        ..rule()
    }
}
fn server_with(ids: &[u64]) -> Value {
    let mut s = server();
    s["server"]["public_net"]["firewalls"] = json!(
        ids.iter()
            .map(|id| json!({"id":id,"status":"applied"}))
            .collect::<Vec<_>>()
    );
    s
}
fn owned(rules: &[Rule], attached: bool) -> Value {
    let mut f = firewall(attached);
    f["rules"] = json!(rules);
    f
}
fn foreign() -> Value {
    let mut f = firewall(true);
    f["id"] = 8.into();
    f["name"] = "original-management".into();
    f["labels"] = json!({"external":"untouched"});
    f["rules"] = json!([
        tcp(22, "198.51.100.0/24"),
        Rule {
            direction: Direction::Out,
            source_ips: vec![],
            destination_ips: vec!["203.0.113.0/24".into()],
            ..tcp(443, "192.0.2.0/24")
        }
    ]);
    f
}
fn snapshot(script: &mut Vec<(u16, Value)>, foreign: Option<&Value>, owned: Option<(&[Rule], bool)>) {
    let mut ids = vec![];
    if foreign.is_some() {
        ids.push(8);
    }
    if owned.is_some_and(|(_, attached)| attached) {
        ids.push(7);
    }
    script.push((200, server_with(&ids)));
    // The production snapshot visits numeric IDs in sorted order.
    if let Some((rules, true)) = owned {
        script.push((200, json!({"firewall":self::owned(rules,true)})));
    }
    if let Some(f) = foreign {
        script.push((200, json!({"firewall":f})));
    }
    if let Some((rules, false)) = owned {
        script.push((200, json!({"firewall":self::owned(rules,false)})));
    }
    script.push((200, server_with(&ids)));
}
fn create(script: &mut Vec<(u16, Value)>, f: Option<&Value>, rules: &[Rule]) {
    snapshot(script, f, None);
    script.push((200, server_with(&if f.is_some() { vec![8] } else { vec![] })));
    script.push((201, json!({"firewall":owned(rules,false),"actions":[]})));
    snapshot(script, f, Some((rules, false)));
    script.push((200, server_with(&if f.is_some() { vec![8] } else { vec![] })));
    script.push((200, json!({"firewall":owned(rules,false)})));
    script.push((201, json!({"actions":[]})));
    snapshot(script, f, Some((rules, true)));
}
fn replace(script: &mut Vec<(u16, Value)>, f: Option<&Value>, old: &[Rule], next: &[Rule]) {
    snapshot(script, f, Some((old, true)));
    script.push((200, server_with(&if f.is_some() { vec![7, 8] } else { vec![7] })));
    script.push((200, json!({"firewall":owned(old,true)})));
    script.push((201, json!({"actions":[]})));
    snapshot(script, f, Some((next, true)));
}
#[derive(Default)]
struct Saved {
    events: Vec<String>,
    fail: Option<&'static str>,
    firewall: Option<u64>,
    requests: Option<Requests>,
    counts: Vec<usize>,
    reconciled: Vec<Firewall>,
}
impl Journal for Saved {
    fn save(&mut self, update: Update<'_>) -> Result<(), CloudError> {
        let name = match update {
            Update::Intent { kind, .. } => format!("intent:{kind:?}"),
            Update::Actions { firewall, actions } => {
                assert_eq!(firewall, self.firewall.unwrap_or(7));
                assert!(actions.len() <= 64);
                "actions".into()
            }
            Update::Reconciled { firewall } => {
                assert_eq!(firewall.id, self.firewall.unwrap_or(7));
                self.reconciled.push(firewall.clone());
                "reconciled".into()
            }
            Update::Confirmed { .. } => "confirmed".into(),
        };
        if let Some(requests) = &self.requests {
            self.counts.push(requests.lock().unwrap().len());
        }
        self.events.push(name.clone());
        if self.fail == Some(name.as_str()) {
            return Err(CloudError::Persistence);
        }
        Ok(())
    }
}

#[test]
fn first_attachment_keeps_explicit_management_all_siblings_and_retires_exact_owned_firewall() {
    let management = tcp(22, "192.0.2.10/32");
    let one = tcp(20000, "192.0.2.0/24");
    let two = tcp(20001, "203.0.113.0/24");
    let a = vec![management.clone(), one.clone()];
    let b = vec![management.clone(), one.clone(), two.clone()];
    let c = vec![management, two.clone()];
    let mut script = vec![];
    snapshot(&mut script, None, None);
    create(&mut script, None, &a);
    replace(&mut script, None, &a, &b);
    replace(&mut script, None, &b, &c);
    snapshot(&mut script, None, Some((&c, true)));
    script.push((200, json!({"firewall":owned(&c,true)})));
    script.push((201, json!({"actions":[]})));
    snapshot(&mut script, None, Some((&c, false)));
    script.push((200, json!({"firewall":owned(&c,false)})));
    script.push((204, Value::Null));
    script.push((404, error("not_found", "absent")));
    snapshot(&mut script, None, None);
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut saved = Saved {
        requests: Some(requests.clone()),
        ..Saved::default()
    };
    let mut manager = client
        .prepare_exposure_management(scope(), self::management(), &cancel)
        .unwrap();
    manager
        .replace_service("service-a", vec![one], &mut saved, &cancel)
        .unwrap();
    manager
        .replace_service("service-b", vec![two], &mut saved, &cancel)
        .unwrap();
    manager.close_service("service-a", &mut saved, &cancel).unwrap();
    manager.close_service("service-b", &mut saved, &cancel).unwrap();
    assert!(!manager.uncertain());
    assert_eq!(manager.firewall_id(), None);
    task.join().unwrap();
    let req = requests.lock().unwrap();
    let mutations: Vec<_> = req.iter().filter(|r| !r.starts_with("GET ")).collect();
    assert_eq!(mutations.len(), 6);
    assert_eq!(request_body(mutations[0])["rules"], json!(a));
    assert_eq!(request_body(mutations[2])["rules"], json!(b));
    assert_eq!(request_body(mutations[3])["rules"], json!(c));
    assert_eq!(
        &saved.events[..5],
        ["intent:Create", "actions", "intent:Apply", "actions", "confirmed"]
    );
    assert_eq!(saved.counts[0], 4);
    assert_eq!(saved.counts[1], 6); // ID save occurs before any next read or wait.
    assert!(mutations.last().unwrap().starts_with("DELETE /firewalls/7 "));
}

#[test]
fn foreign_inbound_and_outbound_policy_is_untouched_and_no_management_allowance_is_added() {
    let f = foreign();
    let rules = vec![rule()];
    let mut script = vec![];
    snapshot(&mut script, Some(&f), None);
    create(&mut script, Some(&f), &rules);
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut saved = Saved::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    manager
        .replace_service("service-a", rules.clone(), &mut saved, &cancel)
        .unwrap();
    task.join().unwrap();
    let req = requests.lock().unwrap();
    let mutations: Vec<_> = req.iter().filter(|r| !r.starts_with("GET ")).collect();
    assert_eq!(mutations.len(), 2);
    assert_eq!(request_body(mutations[0])["rules"], json!(rules));
    assert!(mutations[1].starts_with("POST /firewalls/7/"));
    assert!(!mutations.iter().any(|r| r.contains("/firewalls/8")));
}

#[test]
fn changed_foreign_rules_latch_uncertainty_without_mutation_or_retry() {
    let f = foreign();
    let mut changed = f.clone();
    changed["rules"][0]["source_ips"] = json!(["0.0.0.0/0"]);
    let mut script = vec![];
    snapshot(&mut script, Some(&f), None);
    snapshot(&mut script, Some(&changed), None);
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    let mut saved = Saved::default();
    assert!(
        manager
            .replace_service("service-a", vec![rule()], &mut saved, &cancel)
            .is_err()
    );
    assert!(manager.uncertain());
    assert!(
        manager
            .replace_service("service-a", vec![rule()], &mut saved, &cancel)
            .is_err()
    );
    task.join().unwrap();
    assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
    assert!(saved.events.is_empty());
}

#[test]
fn transitions_missing_attachment_facts_and_selectors_refuse_before_any_effect() {
    for response in [
        server(),
        {
            let mut s = server_with(&[8]);
            s["server"]["public_net"]["firewalls"][0]["status"] = "pending".into();
            s
        },
        server_with(&[8, 8]),
    ] {
        let (client, requests, task) = provider(vec![(200, response)]);
        assert!(
            client
                .prepare_exposure_management(scope(), management(), &Cancellation::default())
                .is_err()
        );
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
    let mut f = foreign();
    f["applied_to"] = json!([{"type":"label_selector","label_selector":{"selector":"all"}}]);
    let (client, requests, task) = provider(vec![(200, server_with(&[8])), (200, json!({"firewall":f}))]);
    assert!(
        client
            .prepare_exposure_management(scope(), management(), &Cancellation::default())
            .is_err()
    );
    task.join().unwrap();
    assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
}

#[test]
fn intent_persistence_failure_prevents_create_and_returned_identity_is_retained_on_save_failure() {
    for fail in ["intent:Create", "actions"] {
        let mut script = vec![];
        snapshot(&mut script, None, None);
        snapshot(&mut script, None, None);
        if fail == "actions" {
            script.push((200, server_with(&[])));
            script.push((
                201,
                json!({"firewall":owned(&[tcp(22,"192.0.2.10/32"),rule()],false),"actions":[action(11,"running")]}),
            ));
        }
        let (client, requests, task) = provider(script);
        let cancel = Cancellation::default();
        let mut manager = client
            .prepare_exposure_management(scope(), management(), &cancel)
            .unwrap();
        let mut saved = Saved {
            fail: Some(fail),
            ..Saved::default()
        };
        assert!(matches!(
            manager.replace_service("service-a", vec![rule()], &mut saved, &cancel),
            Err(CloudError::Persistence)
        ));
        assert!(manager.uncertain());
        assert_eq!(manager.firewall_id(), if fail == "actions" { Some(7) } else { None });
        task.join().unwrap();
        let req = requests.lock().unwrap();
        assert_eq!(
            req.iter().filter(|r| r.starts_with("POST ")).count(),
            usize::from(fail == "actions")
        );
        assert!(!req.iter().any(|r| r.contains("GET /actions/")));
    }
}

#[test]
fn outbound_exposure_contributions_and_cancellation_cannot_dispatch() {
    let mut script = vec![];
    snapshot(&mut script, None, None);
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    let mut saved = Saved::default();
    let mut outbound = rule();
    outbound.direction = Direction::Out;
    outbound.destination_ips = outbound.source_ips.clone();
    outbound.source_ips.clear();
    assert!(
        manager
            .replace_service("service-a", vec![outbound], &mut saved, &cancel)
            .is_err()
    );
    cancel.cancel();
    assert!(matches!(
        manager.replace_service("service-a", vec![rule()], &mut saved, &cancel),
        Err(CloudError::Cancelled)
    ));
    task.join().unwrap();
    assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
    assert!(saved.events.is_empty());
}

#[test]
fn returned_action_ids_are_durable_before_the_first_poll() {
    let rules = vec![tcp(22, "192.0.2.10/32"), rule()];
    let mut script = vec![];
    snapshot(&mut script, None, None);
    create(&mut script, None, &rules);
    script[5].1["actions"] = json!([action(11, "running")]);
    script.insert(6, (200, json!({"action":action(11,"success")})));
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    let mut saved = Saved {
        requests: Some(requests.clone()),
        ..Saved::default()
    };
    manager
        .replace_service("service-a", vec![rule()], &mut saved, &cancel)
        .unwrap();
    task.join().unwrap();
    assert_eq!(saved.events[1], "actions");
    assert_eq!(saved.counts[1], 6);
    assert!(requests.lock().unwrap()[6].starts_with("GET /actions/11 "));
}

#[test]
fn foreign_drift_after_creation_retains_original_id_and_never_applies() {
    let f = foreign();
    let mut changed = f.clone();
    changed["rules"][0]["port"] = "2222".into();
    let rules = vec![rule()];
    let mut script = vec![];
    snapshot(&mut script, Some(&f), None);
    snapshot(&mut script, Some(&f), None);
    script.push((200, server_with(&[8])));
    script.push((201, json!({"firewall":owned(&rules,false),"actions":[]})));
    snapshot(&mut script, Some(&changed), Some((&rules, false)));
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    let mut saved = Saved::default();
    assert!(
        manager
            .replace_service("service-a", rules, &mut saved, &cancel)
            .is_err()
    );
    assert!(manager.uncertain());
    assert_eq!(manager.firewall_id(), Some(7));
    task.join().unwrap();
    let req = requests.lock().unwrap();
    assert_eq!(req.iter().filter(|r| r.starts_with("POST ")).count(), 1);
    assert!(!req.iter().any(|r| r.contains("apply_to_resources")));
    assert_eq!(saved.events, ["intent:Create", "actions"]);
}

#[test]
fn attachment_change_between_snapshot_reads_refuses_without_effect() {
    let (client, requests, task) = provider(vec![(200, server_with(&[])), (200, server_with(&[8]))]);
    assert!(
        client
            .prepare_exposure_management(scope(), management(), &Cancellation::default())
            .is_err()
    );
    task.join().unwrap();
    assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
}

#[test]
fn uncertain_create_reconciles_once_then_exact_cleanup_cannot_reopen() {
    let rules = vec![tcp(22, "192.0.2.10/32"), rule()];
    let mut script = vec![];
    snapshot(&mut script, None, None);
    snapshot(&mut script, None, None);
    script.push((200, server_with(&[])));
    script.push((500, error("server_error", "uncertain")));
    script.push((200, listing("firewalls", json!([owned(&rules, false)]))));
    snapshot(&mut script, None, Some((&rules, false)));
    script.push((200, json!({"firewall":owned(&rules,false)})));
    snapshot(&mut script, None, Some((&rules, false)));
    script.push((200, json!({"firewall":owned(&rules,false)})));
    script.push((204, Value::Null));
    script.push((404, error("not_found", "absent")));
    snapshot(&mut script, None, None);
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    let mut saved = Saved::default();
    assert!(
        manager
            .replace_service("service-a", vec![rule()], &mut saved, &cancel)
            .is_err()
    );
    assert!(manager.uncertain());
    manager.cleanup_original(&mut saved, &cancel).unwrap();
    assert!(!manager.uncertain());
    assert!(
        manager
            .replace_service("service-b", vec![rule()], &mut saved, &cancel)
            .is_err()
    );
    manager.cleanup_original(&mut saved, &cancel).unwrap();
    task.join().unwrap();
    let req = requests.lock().unwrap();
    assert_eq!(req.iter().filter(|r| r.starts_with("POST /firewalls ")).count(), 1);
    assert_eq!(req.iter().filter(|r| r.starts_with("DELETE /firewalls/7 ")).count(), 1);
    assert!(saved.events.contains(&"reconciled".into()));
    assert_eq!(saved.events.last().unwrap(), "confirmed");
}

#[test]
fn uncertain_cleanup_refuses_unexpected_rules_without_touching_the_resource() {
    let rules = vec![tcp(22, "192.0.2.10/32"), rule()];
    let mut script = vec![];
    snapshot(&mut script, None, None);
    snapshot(&mut script, None, None);
    script.push((200, server_with(&[])));
    script.push((201, json!({"firewall":owned(&rules,false),"actions":[]})));
    script.push((200, json!({"firewall":owned(&[tcp(4444,"0.0.0.0/0")],false)})));
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    let mut saved = Saved {
        fail: Some("actions"),
        ..Saved::default()
    };
    assert!(
        manager
            .replace_service("service-a", vec![rule()], &mut saved, &cancel)
            .is_err()
    );
    saved.fail = None;
    assert!(matches!(
        manager.cleanup_original(&mut saved, &cancel),
        Err(CloudError::IdentityMismatch)
    ));
    assert!(manager.uncertain());
    assert_eq!(manager.firewall_id(), Some(7));
    task.join().unwrap();
    let req = requests.lock().unwrap();
    assert_eq!(req.iter().filter(|r| !r.starts_with("GET ")).count(), 1);
    assert!(!req.iter().any(|r| r.starts_with("DELETE ")));
}

#[test]
fn a_live_epoch_removes_last_ingress_without_recreating_its_firewall_for_next_open() {
    let management = tcp(22, "192.0.2.10/32");
    let a = vec![management.clone(), rule()];
    let empty = vec![management.clone()];
    let two = tcp(20001, "203.0.113.0/24");
    let b = vec![management, two.clone()];
    let mut script = vec![];
    snapshot(&mut script, None, None);
    create(&mut script, None, &a);
    replace(&mut script, None, &a, &empty);
    replace(&mut script, None, &empty, &b);
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), self::management(), &cancel)
        .unwrap();
    assert_eq!(manager.ssh_endpoint(), "192.0.2.42:22".parse().unwrap());
    let mut saved = Saved::default();
    manager
        .replace_service("service-a", vec![rule()], &mut saved, &cancel)
        .unwrap();
    manager
        .close_service_retaining_firewall("service-a", &mut saved, &cancel)
        .unwrap();
    assert_eq!(manager.firewall_id(), Some(7));
    manager
        .replace_service("service-b", vec![two], &mut saved, &cancel)
        .unwrap();
    task.join().unwrap();
    let req = requests.lock().unwrap();
    assert_eq!(req.iter().filter(|r| r.starts_with("POST /firewalls ")).count(), 1);
    assert!(!req.iter().any(|r| r.starts_with("DELETE ")));
    let updates: Vec<_> = req.iter().filter(|r| r.contains("/actions/set_rules ")).collect();
    assert_eq!(request_body(updates[0])["rules"], json!(empty));
    assert_eq!(request_body(updates[1])["rules"], json!(b));
}

#[test]
fn original_allocation_server_and_ssh_endpoint_cannot_be_replaced() {
    for field in ["allocation", "server", "ssh"] {
        let mut changed = server_with(&[]);
        match field {
            "allocation" => changed["server"]["labels"]["horizon-operation"] = "other-allocation".into(),
            "server" => changed["server"]["id"] = 43.into(),
            _ => changed["server"]["public_net"]["ipv4"]["ip"] = "192.0.2.43".into(),
        }
        let mut script = vec![];
        snapshot(&mut script, None, None);
        script.push((200, changed.clone()));
        if field == "ssh" {
            script.push((200, changed));
        }
        let (client, requests, task) = provider(script);
        let cancel = Cancellation::default();
        let mut manager = client
            .prepare_exposure_management(scope(), management(), &cancel)
            .unwrap();
        let mut saved = Saved::default();
        assert!(
            manager
                .replace_service("service-a", vec![rule()], &mut saved, &cancel)
                .is_err()
        );
        assert!(manager.uncertain());
        assert!(
            manager
                .replace_service("service-b", vec![rule()], &mut saved, &cancel)
                .is_err()
        );
        task.join().unwrap();
        assert!(
            requests
                .lock()
                .unwrap()
                .iter()
                .all(|request| request.starts_with("GET "))
        );
        assert!(saved.events.is_empty());
    }
}

#[test]
fn management_rules_require_inbound_tcp_and_a_full_attachment_set_refuses() {
    assert!(ManagementRules::new(vec![]).is_err());
    let mut udp = rule();
    udp.protocol = Protocol::Udp;
    assert!(ManagementRules::new(vec![udp]).is_err());
    let mut outbound = rule();
    outbound.direction = Direction::Out;
    outbound.destination_ips = outbound.source_ips.clone();
    outbound.source_ips.clear();
    assert!(ManagementRules::new(vec![outbound]).is_err());
    let ids = [8, 9, 10, 11, 12];
    let mut script = vec![(200, server_with(&ids))];
    for id in ids {
        let mut firewall = foreign();
        firewall["id"] = id.into();
        script.push((200, json!({"firewall":firewall})));
    }
    script.push((200, server_with(&ids)));
    let (client, requests, task) = provider(script);
    assert!(
        client
            .prepare_exposure_management(scope(), management(), &Cancellation::default())
            .is_err()
    );
    task.join().unwrap();
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.starts_with("GET "))
    );
}

fn closed_first_generation(script: &mut Vec<(u16, Value)>, rules: &[Rule]) {
    snapshot(script, None, None);
    create(script, None, rules);
    snapshot(script, None, Some((rules, true)));
    script.push((200, json!({"firewall":owned(rules,true)})));
    script.push((201, json!({"actions":[]})));
    snapshot(script, None, Some((rules, false)));
    script.push((200, json!({"firewall":owned(rules,false)})));
    script.push((204, Value::Null));
    script.push((404, error("not_found", "absent")));
    snapshot(script, None, None);
    for (_, response) in script {
        if let Some(firewall) = response.get_mut("firewall") {
            firewall["id"] = 201.into();
        }
        if let Some(entries) = response
            .pointer_mut("/server/public_net/firewalls")
            .and_then(Value::as_array_mut)
        {
            for entry in entries {
                entry["id"] = 201.into();
            }
        }
    }
}

#[test]
fn a_lost_second_create_reconciles_the_current_generation_not_the_deleted_id() {
    let first = vec![tcp(22, "192.0.2.10/32"), rule()];
    let second = vec![tcp(22, "192.0.2.10/32"), tcp(20001, "203.0.113.0/24")];
    let mut script = vec![];
    closed_first_generation(&mut script, &first);
    snapshot(&mut script, None, None);
    script.push((200, server_with(&[])));
    script.push((500, error("server_error", "lost second create response")));
    let mut found = owned(&second, false);
    found["id"] = 202.into();
    script.push((200, listing("firewalls", json!([found.clone()]))));
    script.push((200, server_with(&[])));
    script.push((200, json!({"firewall":found.clone()})));
    script.push((200, server_with(&[])));
    script.push((200, json!({"firewall":found.clone()})));
    script.push((200, server_with(&[])));
    script.push((200, json!({"firewall":found.clone()})));
    script.push((200, server_with(&[])));
    script.push((200, json!({"firewall":found})));
    script.push((204, Value::Null));
    script.push((404, error("not_found", "absent")));
    snapshot(&mut script, None, None);
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    let mut saved = Saved {
        firewall: Some(201),
        ..Saved::default()
    };
    manager
        .replace_service("service-a", vec![rule()], &mut saved, &cancel)
        .unwrap();
    manager.close_service("service-a", &mut saved, &cancel).unwrap();
    let before = requests.lock().unwrap().len();
    saved.firewall = Some(202);
    assert!(
        manager
            .replace_service("service-b", vec![second[1].clone()], &mut saved, &cancel)
            .is_err()
    );
    assert!(manager.uncertain());
    manager.cleanup_original(&mut saved, &cancel).unwrap();
    assert!(!manager.uncertain());
    assert!(
        manager
            .replace_service("service-c", vec![rule()], &mut saved, &cancel)
            .is_err()
    );
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("POST /firewalls "))
            .count(),
        2
    );
    assert!(
        requests[before..]
            .iter()
            .any(|request| request.starts_with("GET /firewalls?"))
    );
    assert!(
        !requests[before..]
            .iter()
            .any(|request| request.starts_with("GET /firewalls/201 "))
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("DELETE /firewalls/201 "))
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("DELETE /firewalls/202 "))
            .count(),
        1
    );
    assert_eq!(saved.events.last().unwrap(), "confirmed");
}

#[test]
fn a_failed_second_create_intent_retains_the_previous_exact_cleanup_identity() {
    let rules = vec![tcp(22, "192.0.2.10/32"), rule()];
    let mut script = vec![];
    closed_first_generation(&mut script, &rules);
    snapshot(&mut script, None, None);
    script.push((404, error("not_found", "absent")));
    snapshot(&mut script, None, None);
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    let mut saved = Saved {
        firewall: Some(201),
        ..Saved::default()
    };
    manager
        .replace_service("service-a", vec![rule()], &mut saved, &cancel)
        .unwrap();
    manager.close_service("service-a", &mut saved, &cancel).unwrap();
    saved.fail = Some("intent:Create");
    assert!(matches!(
        manager.replace_service("service-b", vec![rule()], &mut saved, &cancel),
        Err(CloudError::Persistence)
    ));
    assert!(manager.uncertain());
    saved.fail = None;
    let before = requests.lock().unwrap().len();
    manager.cleanup_original(&mut saved, &cancel).unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[before].starts_with("GET /firewalls/201 "));
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("POST /firewalls "))
            .count(),
        1
    );
    assert!(!requests.iter().any(|request| request.starts_with("GET /firewalls?")));
}

#[test]
fn unexpected_create_rules_cannot_become_cleanup_policy_after_save_or_wait_failure() {
    for failure in ["actions", "wait"] {
        let unexpected = vec![tcp(4444, "0.0.0.0/0")];
        let mut script = vec![];
        snapshot(&mut script, None, None);
        snapshot(&mut script, None, None);
        script.push((200, server_with(&[])));
        script.push((
            201,
            json!({"firewall":owned(&unexpected,false),"actions":[action(11,"running")]}),
        ));
        if failure == "wait" {
            script.push((
                200,
                json!({"action":{"id":11,"status":"error","error":{"message":"failed"}}}),
            ));
        }
        script.push((200, json!({"firewall":owned(&unexpected,false)})));
        let (client, requests, task) = provider(script);
        let cancel = Cancellation::default();
        let mut manager = client
            .prepare_exposure_management(scope(), management(), &cancel)
            .unwrap();
        let mut saved = Saved {
            fail: (failure == "actions").then_some("actions"),
            ..Saved::default()
        };
        assert!(
            manager
                .replace_service("service-a", vec![rule()], &mut saved, &cancel)
                .is_err()
        );
        assert_eq!(manager.firewall_id(), Some(7));
        assert!(manager.uncertain());
        saved.fail = None;
        assert!(matches!(
            manager.cleanup_original(&mut saved, &cancel),
            Err(CloudError::IdentityMismatch)
        ));
        task.join().unwrap();
        let requests = requests.lock().unwrap();
        assert_eq!(requests.iter().filter(|r| !r.starts_with("GET ")).count(), 1);
        assert_eq!(saved.events, ["intent:Create", "actions"]);
        assert_eq!(
            requests.iter().filter(|r| r.starts_with("GET /actions/11 ")).count(),
            usize::from(failure == "wait")
        );
    }
}

#[test]
fn cleanup_refuses_owned_rule_drift_between_lookup_and_snapshot() {
    let expected = vec![tcp(22, "192.0.2.10/32"), rule()];
    let changed = vec![tcp(4444, "0.0.0.0/0")];
    let mut script = vec![];
    snapshot(&mut script, None, None);
    snapshot(&mut script, None, None);
    script.push((200, server_with(&[])));
    script.push((201, json!({"firewall":owned(&expected,false),"actions":[]})));
    script.push((200, json!({"firewall":owned(&expected,false)})));
    snapshot(&mut script, None, Some((&changed, false)));
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    let mut saved = Saved {
        fail: Some("actions"),
        ..Saved::default()
    };
    assert!(
        manager
            .replace_service("service-a", vec![rule()], &mut saved, &cancel)
            .is_err()
    );
    saved.fail = None;
    assert!(matches!(
        manager.cleanup_original(&mut saved, &cancel),
        Err(CloudError::IdentityMismatch)
    ));
    assert!(manager.uncertain());
    assert_eq!(manager.ssh_endpoint(), "192.0.2.42:22".parse().unwrap());
    task.join().unwrap();
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| !r.starts_with("GET "))
            .count(),
        1
    );
    assert!(saved.reconciled.is_empty());
    assert!(
        !saved
            .events
            .iter()
            .any(|e| e == "intent:Remove" || e == "intent:Delete")
    );
}

#[test]
fn cleanup_never_adopts_a_changed_ssh_endpoint_with_present_or_absent_owned_firewall() {
    for present in [true, false] {
        let rules = vec![tcp(22, "192.0.2.10/32"), rule()];
        let mut script = vec![];
        snapshot(&mut script, None, None);
        snapshot(&mut script, None, None);
        script.push((200, server_with(&[])));
        script.push((201, json!({"firewall":owned(&rules,false),"actions":[]})));
        script.push(if present {
            (200, json!({"firewall":owned(&rules,false)}))
        } else {
            (404, error("not_found", "absent"))
        });
        let start = script.len();
        snapshot(&mut script, None, present.then_some((rules.as_slice(), false)));
        for (_, response) in &mut script[start..] {
            if let Some(ip) = response.pointer_mut("/server/public_net/ipv4/ip") {
                *ip = "192.0.2.43".into();
            }
        }
        let (client, requests, task) = provider(script);
        let cancel = Cancellation::default();
        let mut manager = client
            .prepare_exposure_management(scope(), management(), &cancel)
            .unwrap();
        let mut saved = Saved {
            fail: Some("actions"),
            ..Saved::default()
        };
        assert!(
            manager
                .replace_service("service-a", vec![rule()], &mut saved, &cancel)
                .is_err()
        );
        saved.fail = None;
        assert!(matches!(
            manager.cleanup_original(&mut saved, &cancel),
            Err(CloudError::IdentityMismatch)
        ));
        assert!(manager.uncertain());
        assert_eq!(manager.ssh_endpoint(), "192.0.2.42:22".parse().unwrap());
        assert_eq!(manager.firewall_id(), Some(7));
        task.join().unwrap();
        assert_eq!(
            requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| !r.starts_with("GET "))
                .count(),
            1
        );
        assert!(saved.reconciled.is_empty());
        assert!(
            !saved
                .events
                .iter()
                .any(|e| e == "intent:Remove" || e == "intent:Delete" || e == "confirmed")
        );
    }
}

#[test]
fn failed_verification_latches_uncertainty_even_after_provider_baseline_returns() {
    for read_failure in [true, false] {
        let f = foreign();
        let mut script = vec![];
        snapshot(&mut script, Some(&f), None);
        if read_failure {
            script.push((500, error("server_error", "verification unavailable")));
        } else {
            let mut changed = f.clone();
            changed["rules"][0]["port"] = "2222".into();
            snapshot(&mut script, Some(&changed), None);
        }
        // Independently observe the original provider baseline restored after the failure.
        script.push((200, json!({"firewall":f.clone()})));
        let (client, requests, task) = provider(script);
        let mut observer = Hetzner::new(Credential::new("secret-test-key".into()).unwrap());
        observer.endpoint.clone_from(&client.endpoint);
        let cancel = Cancellation::default();
        let mut manager = client
            .prepare_exposure_management(scope(), management(), &cancel)
            .unwrap();
        assert!(manager.verify_current(&cancel).is_err());
        assert!(manager.uncertain());
        assert_eq!(
            observer.send(Method::Get, "/firewalls/8", None, &cancel).unwrap(),
            json!({"firewall":f})
        );
        let before = requests.lock().unwrap().len();
        let mut saved = Saved::default();
        assert!(matches!(
            manager.replace_service("service-a", vec![rule()], &mut saved, &cancel),
            Err(CloudError::CreationUnresolved)
        ));
        assert!(matches!(
            manager.verify_current(&cancel),
            Err(CloudError::CreationUnresolved)
        ));
        task.join().unwrap();
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), before);
        assert!(requests.iter().all(|r| r.starts_with("GET ")));
        assert!(saved.events.is_empty());
    }
}

#[test]
fn cleanup_journals_the_freshly_captured_attachment_before_retirement() {
    let rules = vec![tcp(22, "192.0.2.10/32"), rule()];
    let mut script = vec![];
    snapshot(&mut script, None, None);
    snapshot(&mut script, None, None);
    script.push((200, server_with(&[])));
    script.push((201, json!({"firewall":owned(&rules,false),"actions":[]})));
    script.push((200, json!({"firewall":owned(&rules,false)})));
    snapshot(&mut script, None, Some((&rules, true)));
    script.push((200, json!({"firewall":owned(&rules,true)})));
    script.push((201, json!({"actions":[]})));
    snapshot(&mut script, None, Some((&rules, false)));
    script.push((200, json!({"firewall":owned(&rules,false)})));
    script.push((204, Value::Null));
    script.push((404, error("not_found", "absent")));
    snapshot(&mut script, None, None);
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    let mut saved = Saved {
        fail: Some("actions"),
        ..Saved::default()
    };
    assert!(
        manager
            .replace_service("service-a", vec![rule()], &mut saved, &cancel)
            .is_err()
    );
    saved.fail = None;
    manager.cleanup_original(&mut saved, &cancel).unwrap();
    task.join().unwrap();
    assert_eq!(saved.reconciled.len(), 1);
    assert_eq!(saved.reconciled[0].rules, rules);
    assert_eq!(saved.reconciled[0].applied_to.len(), 1);
    assert_eq!(saved.reconciled[0].applied_to[0].server.as_ref().unwrap().id, 42);
    let requests = requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.contains("/actions/remove_from_resources "))
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.starts_with("DELETE /firewalls/7 "))
            .count(),
        1
    );
    assert!(!manager.uncertain());
}

fn cleanup_without_a_create_attempt_preserves_the_existing_match(failure: &str) {
    let rules = vec![tcp(22, "192.0.2.10/32"), rule()];
    let existing = owned(&rules, false);
    let mut script = vec![];
    snapshot(&mut script, None, None);
    // An independent read proves this unattached exact-scope match predates the attempt.
    script.push((200, listing("firewalls", json!([existing.clone()]))));
    let compare = script.len();
    snapshot(&mut script, None, None);
    if failure == "compare" {
        for (_, response) in &mut script[compare..] {
            response["server"]["public_net"]["ipv4"]["ip"] = "192.0.2.43".into();
        }
    } else if failure == "preflight" {
        script.push((500, error("server_error", "original server preflight unavailable")));
    } else if failure != "intent" {
        script.push((200, server_with(&[])));
        script.push(if failure == "unique" {
            (409, error("uniqueness_error", "name is already in use"))
        } else {
            (400, error("invalid_input", "create refused"))
        });
    }
    // These first replies work for a snapshot and also expose the old unsafe scoped lookup.
    let mut both = server_with(&[]);
    both["firewalls"] = json!([existing.clone()]);
    both["meta"] = json!({"pagination":{"next_page":null}});
    script.push((200, both));
    script.push((200, server_with(&[])));
    script.push((200, json!({"firewall":existing.clone()})));
    // The old implementation can continue through exact adoption and deletion.
    script.push((200, server_with(&[])));
    script.push((200, json!({"firewall":existing.clone()})));
    snapshot(&mut script, None, Some((&rules, false)));
    script.push((200, json!({"firewall":existing})));
    script.push((204, Value::Null));
    script.push((404, error("not_found", "deleted pre-existing match")));
    snapshot(&mut script, None, None);
    script.push((404, error("not_found", "deleted pre-existing match")));
    let (client, requests, task) = provider(script);
    let mut observer = Hetzner::new(Credential::new("secret-test-key".into()).unwrap());
    observer.endpoint.clone_from(&client.endpoint);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    assert_eq!(
        observer.find_exposure_firewall(&scope(), &cancel).unwrap().unwrap().id,
        7
    );
    let mut saved = Saved {
        fail: (failure == "intent").then_some("intent:Create"),
        ..Saved::default()
    };
    assert!(
        manager
            .replace_service("service-a", vec![rule()], &mut saved, &cancel)
            .is_err()
    );
    assert!(manager.uncertain());
    assert_eq!(manager.firewall_id(), None);
    saved.fail = None;
    let before = requests.lock().unwrap().len();
    let cleanup = manager.cleanup_original(&mut saved, &cancel);
    let remaining = observer.inspect_exposure_firewall(&scope(), 7, &cancel);
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(
        !requests[before..].iter().any(|r| r.starts_with("GET /firewalls?")),
        "Cleanup searched without an unresolved create: {failure}; destructive requests: {:?}",
        requests
            .iter()
            .filter(|r| r.starts_with("DELETE ") || r.contains("remove_from_resources"))
            .map(|r| r.lines().next().unwrap())
            .collect::<Vec<_>>()
    );
    assert!(cleanup.is_ok(), "{failure}: {cleanup:?}");
    assert_eq!(remaining.unwrap().unwrap().id, 7);
    assert!(saved.reconciled.is_empty());
    assert!(!manager.uncertain());
    assert!(
        !requests
            .iter()
            .any(|r| r.starts_with("DELETE ") || r.contains("remove_from_resources"))
    );
    assert_eq!(
        requests.iter().filter(|r| r.starts_with("POST /firewalls ")).count(),
        usize::from(failure == "unique" || failure == "rejected")
    );
}

#[test]
fn failed_first_create_intent_cannot_adopt_a_preexisting_unattached_match() {
    cleanup_without_a_create_attempt_preserves_the_existing_match("intent");
}
#[test]
fn failed_initial_compare_cannot_adopt_a_preexisting_unattached_match() {
    cleanup_without_a_create_attempt_preserves_the_existing_match("compare");
}
#[test]
fn failed_create_server_preflight_cannot_adopt_a_preexisting_unattached_match() {
    cleanup_without_a_create_attempt_preserves_the_existing_match("preflight");
}
#[test]
fn refused_create_name_conflict_cannot_adopt_a_preexisting_unattached_match() {
    cleanup_without_a_create_attempt_preserves_the_existing_match("unique");
}
#[test]
fn definitely_rejected_create_cannot_adopt_a_preexisting_unattached_match() {
    cleanup_without_a_create_attempt_preserves_the_existing_match("rejected");
}

#[test]
fn failed_second_create_preflight_retains_the_previous_exact_cleanup_identity() {
    let rules = vec![tcp(22, "192.0.2.10/32"), rule()];
    let mut script = vec![];
    closed_first_generation(&mut script, &rules);
    snapshot(&mut script, None, None);
    script.push((500, error("server_error", "second create preflight unavailable")));
    script.push((404, error("not_found", "original generation absent")));
    snapshot(&mut script, None, None);
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    let mut saved = Saved {
        firewall: Some(201),
        ..Saved::default()
    };
    manager
        .replace_service("service-a", vec![rule()], &mut saved, &cancel)
        .unwrap();
    manager.close_service("service-a", &mut saved, &cancel).unwrap();
    assert!(
        manager
            .replace_service("service-b", vec![rule()], &mut saved, &cancel)
            .is_err()
    );
    assert!(manager.uncertain());
    let before = requests.lock().unwrap().len();
    manager.cleanup_original(&mut saved, &cancel).unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[before].starts_with("GET /firewalls/201 "));
    assert!(!requests.iter().any(|r| r.starts_with("GET /firewalls?")));
    assert_eq!(requests.iter().filter(|r| r.starts_with("POST /firewalls ")).count(), 1);
}

#[test]
fn an_empty_lookup_after_an_unknown_create_remains_unresolved_until_the_original_appears() {
    let rules = vec![tcp(22, "192.0.2.10/32"), rule()];
    let mut script = vec![];
    snapshot(&mut script, None, None);
    snapshot(&mut script, None, None);
    script.push((200, server_with(&[])));
    script.push((500, error("server_error", "original create response lost")));
    script.push((200, listing("firewalls", json!([]))));
    // A delayed response can become visible only after the first empty lookup.
    // The server fields also permit the old false-absence path to complete.
    let mut later = server_with(&[]);
    later["firewalls"] = json!([owned(&rules, false)]);
    later["meta"] = json!({"pagination":{"next_page":null}});
    script.push((200, later));
    snapshot(&mut script, None, Some((&rules, false)));
    script.push((200, json!({"firewall":owned(&rules,false)})));
    snapshot(&mut script, None, Some((&rules, false)));
    script.push((200, json!({"firewall":owned(&rules,false)})));
    script.push((204, Value::Null));
    script.push((404, error("not_found", "original deleted")));
    snapshot(&mut script, None, None);
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    let mut saved = Saved::default();
    assert!(
        manager
            .replace_service("service-a", vec![rule()], &mut saved, &cancel)
            .is_err()
    );
    let before = requests.lock().unwrap().len();
    let first_cleanup = manager.cleanup_original(&mut saved, &cancel);
    assert!(
        matches!(first_cleanup, Err(CloudError::CreationUnresolved)),
        "Empty lookup falsely resolved: {first_cleanup:?}; durable events: {:?}",
        saved.events
    );
    assert!(manager.uncertain());
    assert_eq!(manager.firewall_id(), None);
    assert_eq!(saved.events, ["intent:Create"]);
    assert_eq!(requests.lock().unwrap().len(), before + 1);
    assert!(matches!(
        manager.replace_service("service-b", vec![rule()], &mut saved, &cancel),
        Err(CloudError::CreationUnresolved)
    ));
    assert_eq!(requests.lock().unwrap().len(), before + 1);
    manager.cleanup_original(&mut saved, &cancel).unwrap();
    assert!(!manager.uncertain());
    assert!(matches!(
        manager.replace_service("service-b", vec![rule()], &mut saved, &cancel),
        Err(CloudError::CreationUnresolved)
    ));
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.iter().filter(|r| r.starts_with("POST /firewalls ")).count(), 1);
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.starts_with("DELETE /firewalls/7 "))
            .count(),
        1
    );
    assert_eq!(saved.events.last().unwrap(), "confirmed");
}

fn rejected_candidate_keeps_the_original_request_unresolved(failed_save: bool) {
    let rules = vec![tcp(22, "192.0.2.10/32"), rule()];
    let mut script = vec![];
    snapshot(&mut script, None, None);
    snapshot(&mut script, None, None);
    script.push((200, server_with(&[])));
    script.push((500, error("server_error", "original create reply lost")));
    script.push((200, listing("firewalls", json!([owned(&rules, false)]))));
    if failed_save {
        snapshot(&mut script, None, Some((&rules, false)));
    } else {
        script.push((200, server_with(&[])));
        script.push((404, error("not_found", "stale lookup candidate")));
    }
    script.push((200, listing("firewalls", json!([]))));
    let id = if failed_save { 7 } else { 8 };
    let mut found = owned(&rules, false);
    found["id"] = id.into();
    script.push((200, listing("firewalls", json!([found]))));
    let later = script.len();
    snapshot(&mut script, None, Some((&rules, false)));
    script.push((200, json!({"firewall":owned(&rules,false)})));
    snapshot(&mut script, None, Some((&rules, false)));
    script.push((200, json!({"firewall":owned(&rules,false)})));
    script.push((204, Value::Null));
    script.push((404, error("not_found", "original retired")));
    snapshot(&mut script, None, None);
    for (_, response) in &mut script[later..] {
        if let Some(firewall) = response.get_mut("firewall") {
            firewall["id"] = id.into();
        }
    }
    let (client, requests, task) = provider(script);
    let cancel = Cancellation::default();
    let mut manager = client
        .prepare_exposure_management(scope(), management(), &cancel)
        .unwrap();
    let mut saved = Saved::default();
    assert!(
        manager
            .replace_service("service-a", vec![rule()], &mut saved, &cancel)
            .is_err()
    );
    saved.fail = failed_save.then_some("reconciled");
    let first = manager.cleanup_original(&mut saved, &cancel);
    if failed_save {
        assert!(matches!(first, Err(CloudError::Persistence)));
    } else {
        assert!(first.is_err());
    }
    assert!(manager.uncertain());
    assert_eq!(manager.firewall_id(), None);
    saved.fail = None;
    let before = requests.lock().unwrap().len();
    let second = manager.cleanup_original(&mut saved, &cancel);
    let requests_so_far = requests.lock().unwrap();
    assert!(
        requests_so_far[before].starts_with("GET /firewalls?"),
        "Unvalidated or unsaved candidate became an exact cleanup identity: {}",
        requests_so_far[before].lines().next().unwrap()
    );
    drop(requests_so_far);
    assert!(matches!(second, Err(CloudError::CreationUnresolved)));
    assert!(manager.uncertain());
    assert!(!saved.events.iter().any(|event| event == "confirmed"));
    assert!(matches!(
        manager.replace_service("service-b", vec![rule()], &mut saved, &cancel),
        Err(CloudError::CreationUnresolved)
    ));
    saved.firewall = Some(id);
    manager.cleanup_original(&mut saved, &cancel).unwrap();
    assert!(!manager.uncertain());
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.iter().filter(|r| r.starts_with("POST /firewalls ")).count(), 1);
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.starts_with(&format!("DELETE /firewalls/{id} ")))
            .count(),
        1
    );
    if id == 8 {
        assert!(!requests.iter().any(|r| r.starts_with("DELETE /firewalls/7 ")));
    }
    assert_eq!(saved.events.last().unwrap(), "confirmed");
}

#[test]
fn a_stale_unknown_create_candidate_cannot_replace_the_unresolved_request() {
    rejected_candidate_keeps_the_original_request_unresolved(false);
}
#[test]
fn a_failed_reconciliation_save_cannot_promote_the_unknown_create_candidate() {
    rejected_candidate_keeps_the_original_request_unresolved(true);
}

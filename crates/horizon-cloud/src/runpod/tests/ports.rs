use super::*;
use std::collections::BTreeMap;

fn inspect(value: &Value) -> Worker {
    let (provider, requests, task) = server(vec![(200, value.to_string())]);
    let found = provider.inspect("worker1", &Cancellation::default()).unwrap().unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /pods/worker1 "));
    found
}

#[test]
fn actual_tcp_mappings_are_opt_in_and_preserve_the_original_ssh_endpoint() {
    for host in ["192.0.2.1", "2001:db8::1"] {
        let mut value = worker(&spec());
        value["ssh"]["direct"]["host"] = host.into();
        value["runtime"] = json!({"uptime":30,"ports":[
            {"private":22,"public":22001,"type":"tcp","ip":host},
            {"private":20000,"public":33000,"type":"tcp","ip":host},
            {"private":8888,"public":33001,"type":"http","ip":host},
            {"private":20001,"public":null,"type":"tcp","ip":null}
        ]});
        let found = inspect(&value);
        let ports = found.port_mappings.as_ref().unwrap();
        assert_eq!(ports["22"], 22001);
        assert_eq!(found.ssh_endpoint().unwrap().port(), 22001);
        assert_eq!(
            ports.get("20000").copied(),
            cfg!(feature = "exposure-ports").then_some(33000)
        );
        assert!(!ports.contains_key("8888"));
        assert!(!ports.contains_key("20001"));
        assert_eq!(found.public_ip, Some(host.parse().unwrap()));
    }
}

#[test]
fn ambiguous_or_invalid_runtime_facts_do_not_supply_exposure_endpoints() {
    let valid = json!({"private":20000,"public":33000,"type":"tcp","ip":"192.0.2.1"});
    for runtime in [
        Value::Null,
        json!({}),
        json!({"ports":[{"private":0,"public":33000,"type":"tcp","ip":"192.0.2.1"}]}),
        json!({"ports":[{"private":20000,"public":0,"type":"tcp","ip":"192.0.2.1"}]}),
        json!({"ports":[{"private":20000,"public":65536,"type":"tcp","ip":"192.0.2.1"}]}),
        json!({"ports":[{"private":20000,"public":33000,"type":"tcp","ip":"192.0.2.2"}]}),
        json!({"ports":[valid.clone(),valid.clone()]}),
        json!({"ports":[valid.clone(),{"private":20001,"public":33000,"type":"tcp","ip":"192.0.2.1"}]}),
        json!({"ports":[valid.clone(),{"private":22,"public":22002,"type":"tcp","ip":"192.0.2.1"}]}),
        json!({"ports":[valid.clone(),{"private":20001,"public":33001,"type":"unknown","ip":"192.0.2.1"}]}),
        json!({"ports":[valid.clone(),{"private":20001,"public":33001,"type":"","ip":"192.0.2.1"}]}),
        json!({"ports":[{"private":20000,"public":33000,"type":"tcp","ip":"192.0.2.1","future_semantics":true}]}),
        json!({"ports":vec![valid;129]}),
    ] {
        let mut value = worker(&spec());
        value["runtime"] = runtime;
        let found = inspect(&value);
        assert_eq!(found.port_mappings.unwrap(), BTreeMap::from([("22".into(), 22001)]));
        assert_eq!(found.public_ip, Some("192.0.2.1".parse().unwrap()));
    }
}

#[test]
fn stopped_or_dns_ssh_endpoints_do_not_promote_runtime_ports() {
    for (status, host) in [("EXITED", "192.0.2.1"), ("RUNNING", "direct.example.test")] {
        let mut value = worker(&spec());
        value["status"] = status.into();
        value["ssh"]["direct"]["host"] = host.into();
        value["runtime"] = json!({"ports":[{"private":20000,"public":33000,"type":"tcp","ip":"192.0.2.1"}]});
        assert_eq!(
            inspect(&value).port_mappings.unwrap(),
            BTreeMap::from([("22".into(), 22001)])
        );
    }
}

#[test]
fn requested_ports_without_live_public_mappings_are_not_endpoints() {
    let mut value = worker(&spec());
    value["ports"] = json!(["22/tcp", "20000/tcp"]);
    assert_eq!(
        inspect(&value).port_mappings.unwrap(),
        BTreeMap::from([("22".into(), 22001)])
    );
}

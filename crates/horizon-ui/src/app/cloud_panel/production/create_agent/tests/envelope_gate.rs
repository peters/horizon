use super::*;
use crate::app::cloud_panel::production::create_agent::envelope_gate::{LEDGER, NO_ENVELOPE, admit};
use horizon_core::cloud_runtime::envelope;

fn approved() -> Value {
    json!({
        "provider": "runpod", "currency": "USD", "max_hours": 6.0, "max_total": 2.0,
        "approved_by": "peters", "approved_at": "2026-11-02T09:00:00Z",
    })
}

fn ask(hours: f64, estimate: f64) -> envelope::Ask {
    envelope::Ask {
        provider: "runpod".into(),
        currency: "USD".into(),
        hours,
        estimate,
    }
}

#[test]
fn a_machine_without_a_spending_envelope_rents_nothing() {
    let (temp, mut app, _workspace, actor) = agent_app(Some(policy()));
    let request = build_request(
        &actor,
        manifest::host_instance(),
        submit("/home/person/shop"),
        manifest::now_millis() + 10_000,
    );
    assert_eq!(answer(&mut app, &request).unwrap()["phase"], json!("submitted"));
    let ctx = Context::default();
    finish(&mut app, &ctx);
    let operation = &app.cloud_prototype.production.create_agent.operations[ID];
    assert_eq!(operation.phase, Phase::Failed);
    assert_eq!(operation.error.as_deref(), Some(NO_ENVELOPE));
    assert!(app.cloud_prototype.groups.0.is_empty(), "nothing is registered");
    assert!(
        !temp.path().join("cloud").join("envelope-rents.jsonl").exists(),
        "a refused create records no rent"
    );
}

#[test]
fn an_envelope_that_is_exhausted_records_nothing_and_names_the_limit() {
    let (temp, _app, _workspace, _actor) = agent_app(Some(policy()));
    let root = temp.path().join("cloud");
    let approved: envelope::Envelope = serde_json::from_value(approved()).expect("the section decodes");
    let admitted = admit(&root, &approved, &ask(4.0, 1.5), "cloud-1", 0.03, "horizon:agent").unwrap();
    assert_eq!((admitted.hours, admitted.estimate), (4.0, 1.5));
    let rents = envelope::Ledger::at(&root).rents().unwrap();
    assert_eq!(rents.len(), 1);
    assert_eq!(
        (
            rents[0].cloud.as_str(),
            rents[0].actor.as_str(),
            rents[0].approved_by.as_str()
        ),
        ("cloud-1", "horizon:agent", "peters")
    );

    // Two hours are left and half the money: a longer or dearer rent is refused, and the
    // refusal names what the envelope allows and what is left.
    let refused = admit(&root, &approved, &ask(3.0, 0.1), "cloud-2", 0.03, "horizon:agent").unwrap_err();
    assert!(refused.starts_with("cloud_create_envelope_hours"), "{refused}");
    assert!(refused.contains("2 h and 0.50 USD are left"), "{refused}");
    let refused = admit(&root, &approved, &ask(1.0, 0.6), "cloud-2", 0.03, "horizon:agent").unwrap_err();
    assert!(refused.starts_with("cloud_create_envelope_amount"), "{refused}");
    assert_eq!(
        envelope::Ledger::at(&root).rents().unwrap().len(),
        1,
        "a refused rent writes nothing"
    );

    // What the first cloud left is enough for one more small cloud, and no more.
    admit(&root, &approved, &ask(2.0, 0.5), "cloud-2", 0.03, "horizon:agent").unwrap();
    assert_eq!(envelope::Ledger::at(&root).rents().unwrap().len(), 2);
    let refused = admit(&root, &approved, &ask(1.0, 0.1), "cloud-3", 0.03, "horizon:agent").unwrap_err();
    assert!(refused.starts_with("cloud_create_envelope_hours"), "{refused}");
}

#[test]
fn an_offer_on_another_provider_or_in_another_currency_is_outside_the_approved() {
    let (temp, _app, _workspace, _actor) = agent_app(Some(policy()));
    let root = temp.path().join("cloud");
    let approved: envelope::Envelope = serde_json::from_value(approved()).expect("the section decodes");
    for (provider, currency) in [("hetzner", "USD"), ("runpod", "EUR")] {
        let other = envelope::Ask {
            provider: provider.into(),
            currency: currency.into(),
            hours: 1.0,
            estimate: 0.1,
        };
        let refused = admit(&root, &approved, &other, "cloud-1", 0.03, "horizon:agent").unwrap_err();
        assert!(refused.starts_with("cloud_create_outside_envelope"), "{refused}");
        assert!(
            refused.contains("this offer is on") || refused.contains("bills in"),
            "{refused}"
        );
    }
    assert!(envelope::Ledger::at(&root).rents().unwrap().is_empty());
}

#[test]
fn a_machine_whose_envelope_record_cannot_be_read_rents_nothing() {
    let (temp, mut app, _workspace, actor) = agent_app_with_envelope(Some(policy()), Some(approved()));
    let root = temp.path().join("cloud");
    std::fs::write(root.join("envelope-rents.jsonl"), "x".repeat(300 * 1024)).unwrap();
    let request = build_request(
        &actor,
        manifest::host_instance(),
        submit("/home/person/shop"),
        manifest::now_millis() + 10_000,
    );
    assert_eq!(answer(&mut app, &request).unwrap()["phase"], json!("submitted"));
    let ctx = Context::default();
    finish(&mut app, &ctx);
    let operation = &app.cloud_prototype.production.create_agent.operations[ID];
    assert_eq!(operation.error.as_deref(), Some(LEDGER));
    assert!(app.cloud_prototype.groups.0.is_empty());
}

use super::*;

fn envelope() -> Envelope {
    Envelope {
        provider: "hetzner".into(),
        currency: "EUR".into(),
        max_hours: 6.0,
        max_total: 2.0,
        approved_by: "peters".into(),
        approved_at: "2026-11-02T09:00:00Z".into(),
    }
}

fn ask(provider: &str, currency: &str, hours: f64, estimate: f64) -> Ask {
    Ask {
        provider: provider.into(),
        currency: currency.into(),
        hours,
        estimate,
    }
}

fn rent(cloud: &str, hours: f64, estimate: f64) -> Rent {
    Rent {
        schema_version: 1,
        at_millis: 1_700_000_000_000,
        cloud: cloud.into(),
        provider: "hetzner".into(),
        hours,
        hourly: 0.005,
        estimate,
        currency: "EUR".into(),
        approved_by: "peters".into(),
        approved_at: "2026-11-02T09:00:00Z".into(),
        actor: "horizon:agent".into(),
    }
}

/// A rent of an approval that a later envelope replaced.
fn earlier_rent(cloud: &str, hours: f64, estimate: f64) -> Rent {
    Rent {
        approved_at: "2026-11-01T09:00:00Z".into(),
        currency: "USD".into(),
        provider: "runpod".into(),
        ..rent(cloud, hours, estimate)
    }
}

#[test]
fn a_rent_inside_the_envelope_is_admitted_with_what_it_costs() {
    let envelope = envelope();
    let admitted = envelope
        .admit(Spent::default(), &ask("hetzner", "EUR", 4.0, 1.5))
        .unwrap();
    assert_eq!(
        (admitted.hours, admitted.estimate, admitted.currency.as_str()),
        (4.0, 1.5, "EUR")
    );
    assert_eq!(admitted.approved_by, "peters");
    // An amount or a duration exactly at the limit is inside it.
    assert!(
        envelope
            .admit(Spent::default(), &ask("hetzner", "EUR", 6.0, 2.0))
            .is_ok()
    );
}

#[test]
fn each_limit_is_named_in_its_own_code_and_sentence() {
    let envelope = envelope();
    let cases = [
        (ask("runpod", "EUR", 4.0, 1.0), Refusal::Provider),
        (ask("hetzner", "USD", 4.0, 1.0), Refusal::Currency),
        (ask("hetzner", "EUR", 7.0, 1.0), Refusal::Hours),
        (ask("hetzner", "EUR", 0.0, 1.0), Refusal::Hours),
        (ask("hetzner", "EUR", f64::NAN, 1.0), Refusal::Hours),
        (ask("hetzner", "EUR", 4.0, 2.01), Refusal::Amount),
        (ask("hetzner", "EUR", 4.0, f64::NAN), Refusal::Amount),
    ];
    for (ask, refusal) in cases {
        let refused = envelope.admit(Spent::default(), &ask).unwrap_err();
        assert_eq!(refused, refusal, "{ask:?}");
        let sentence = refused.describe(&envelope, Spent::default(), &ask);
        assert!(
            sentence.starts_with("this envelope allows EUR in total on hetzner up to 6 h and 2.00 EUR")
                && sentence.contains("6 h and 2.00 EUR are left"),
            "{sentence}"
        );
    }
    assert_eq!(Refusal::Provider.code(), "cloud_create_outside_envelope");
    assert_eq!(Refusal::Hours.code(), "cloud_create_envelope_hours");
    assert_eq!(Refusal::Amount.code(), "cloud_create_envelope_amount");
}

#[test]
fn what_the_envelope_already_paid_for_counts_against_the_next_rent() {
    let envelope = envelope();
    let spent = Spent {
        hours: 5.0,
        amount: 1.5,
    };
    assert_eq!(envelope.remaining(spent).hours, 1.0);
    assert_eq!(envelope.remaining(spent).amount, 0.5);
    // A second cloud of the same envelope may use what is left, and no more.
    assert!(envelope.admit(spent, &ask("hetzner", "EUR", 1.0, 0.5)).is_ok());
    assert_eq!(
        envelope.admit(spent, &ask("hetzner", "EUR", 1.1, 0.1)).unwrap_err(),
        Refusal::Hours
    );
    assert_eq!(
        envelope.admit(spent, &ask("hetzner", "EUR", 1.0, 0.6)).unwrap_err(),
        Refusal::Amount
    );
    let exhausted = Spent {
        hours: 6.0,
        amount: 2.0,
    };
    assert_eq!(envelope.remaining(exhausted).hours, 0.0);
    assert_eq!(envelope.remaining(exhausted).amount, 0.0);
    assert_eq!(
        envelope.admit(exhausted, &ask("hetzner", "EUR", 1.0, 0.1)).unwrap_err(),
        Refusal::Hours
    );
}

#[test]
fn an_envelope_must_name_what_it_allows_and_who_approved_it() {
    assert_eq!(envelope().validate(), Ok(()));
    let cases = [
        (
            Envelope {
                provider: " ".into(),
                ..envelope()
            },
            Invalid::Provider,
        ),
        (
            Envelope {
                currency: "eur".into(),
                ..envelope()
            },
            Invalid::Currency,
        ),
        (
            Envelope {
                currency: "EURO".into(),
                ..envelope()
            },
            Invalid::Currency,
        ),
        (
            Envelope {
                max_hours: 0.0,
                ..envelope()
            },
            Invalid::Hours,
        ),
        (
            Envelope {
                max_hours: LONGEST_HOURS + 1.0,
                ..envelope()
            },
            Invalid::Hours,
        ),
        (
            Envelope {
                max_total: 0.0,
                ..envelope()
            },
            Invalid::Total,
        ),
        (
            Envelope {
                approved_by: String::new(),
                ..envelope()
            },
            Invalid::Approval,
        ),
        (
            Envelope {
                approved_at: "yesterday".into(),
                ..envelope()
            },
            Invalid::Approval,
        ),
    ];
    for (envelope, invalid) in cases {
        assert_eq!(envelope.validate(), Err(invalid));
        assert!(!invalid.describe().is_empty());
    }
}

#[test]
fn the_ledger_keeps_one_line_per_rent_and_answers_what_was_spent() {
    let temp = tempfile::tempdir().unwrap();
    let ledger = Ledger::at(temp.path());
    assert_eq!(
        ledger.spent(&envelope()).unwrap(),
        Spent::default(),
        "no rent until one is admitted"
    );

    ledger.record(&rent("cloud-1", 2.0, 0.5)).unwrap();
    ledger.record(&rent("cloud-2", 1.5, 0.25)).unwrap();
    let rents = ledger.rents().unwrap();
    assert_eq!(rents.len(), 2);
    assert_eq!(rents[1].cloud, "cloud-2");
    assert_eq!(rents[1].approved_by, "peters");
    let spent = ledger.spent(&envelope()).unwrap();
    assert!((spent.hours - 3.5).abs() < 1e-9);
    assert!((spent.amount - 0.75).abs() < 1e-9);
    // The record is one line per rent, and it holds no credential.
    let text = std::fs::read_to_string(ledger.path()).unwrap();
    assert_eq!(text.lines().count(), 2);
    assert!(!text.contains("key") && !text.contains("token"), "{text}");
}

#[test]
fn a_line_that_a_crash_left_half_written_changes_no_bound_and_never_swallows_the_next() {
    let temp = tempfile::tempdir().unwrap();
    let ledger = Ledger::at(temp.path());
    ledger.record(&rent("cloud-1", 1.0, 0.1)).unwrap();
    let mut text = std::fs::read_to_string(ledger.path()).unwrap();
    text.push_str("{\"schema_version\":1,\"cloud\":\"cloud-2\"");
    std::fs::write(ledger.path(), text).unwrap();
    let rents = ledger.rents().unwrap();
    assert_eq!(rents.len(), 1, "the half-written tail is not a rent");
    assert!((ledger.spent(&envelope()).unwrap().hours - 1.0).abs() < 1e-9);

    // The next admitted rent is a line of its own: the torn tail is dropped first, so the
    // record never holds two records joined into one line that no reader can count.
    ledger.record(&rent("cloud-2", 2.0, 0.2)).unwrap();
    let rents = ledger.rents().unwrap();
    assert_eq!(rents.len(), 2, "{rents:?}");
    assert_eq!(rents[1].cloud, "cloud-2");
    assert!((ledger.spent(&envelope()).unwrap().hours - 3.0).abs() < 1e-9);
    let text = std::fs::read_to_string(ledger.path()).unwrap();
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(text.ends_with('\n'));
}

#[test]
fn a_line_horizon_cannot_read_admits_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let ledger = Ledger::at(temp.path());
    ledger.record(&rent("cloud-1", 1.0, 0.1)).unwrap();
    let mut text = std::fs::read_to_string(ledger.path()).unwrap();
    text.push_str("not a rent\n");
    std::fs::write(ledger.path(), text).unwrap();
    assert!(matches!(ledger.rents(), Err(Error::Invalid(_))));
    assert!(matches!(ledger.spent(&envelope()), Err(Error::Invalid(_))));
}

#[test]
fn a_record_with_more_rents_than_horizon_counts_admits_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let ledger = Ledger::at(temp.path());
    let mut text = String::new();
    for index in 0..=MAX_LEDGER_RENTS {
        text.push_str(&serde_json::to_string(&rent(&format!("cloud-{index}"), 0.01, 0.001)).unwrap());
        text.push('\n');
    }
    assert!(
        u64::try_from(text.len()).unwrap() < MAX_LEDGER_BYTES,
        "the count bound is what stops it"
    );
    std::fs::write(ledger.path(), text).unwrap();
    assert!(matches!(ledger.rents(), Err(Error::Invalid(_))));
}

#[test]
fn a_later_approval_counts_only_its_own_rents() {
    let temp = tempfile::tempdir().unwrap();
    let ledger = Ledger::at(temp.path());
    // One cloud paid for by an earlier approval, in another currency and on another provider.
    ledger.record(&earlier_rent("cloud-0", 5.0, 1.9)).unwrap();
    assert_eq!(
        ledger.spent(&envelope()).unwrap(),
        Spent::default(),
        "a new approval starts with its whole budget"
    );
    ledger.record(&rent("cloud-1", 2.0, 0.5)).unwrap();
    let spent = ledger.spent(&envelope()).unwrap();
    assert!((spent.hours - 2.0).abs() < 1e-9);
    assert!((spent.amount - 0.5).abs() < 1e-9);
    // The earlier approval is still readable in the record, so a later audit keeps it.
    assert_eq!(ledger.rents().unwrap().len(), 2);
}

#[test]
fn a_ledger_longer_than_horizon_reads_admits_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let ledger = Ledger::at(temp.path());
    let over = usize::try_from(MAX_LEDGER_BYTES).unwrap() + 1;
    std::fs::write(ledger.path(), "x".repeat(over)).unwrap();
    assert!(matches!(ledger.rents(), Err(Error::Invalid(_))));
    assert!(matches!(ledger.spent(&envelope()), Err(Error::Invalid(_))));
    assert!(
        matches!(ledger.record(&rent("", 1.0, 0.1)), Err(Error::Invalid(_))),
        "a rent without a cloud is refused"
    );
}

#[test]
fn the_approval_reads_back_as_it_was_written() {
    let text = serde_json::to_string(&envelope()).unwrap();
    assert_eq!(serde_json::from_str::<Envelope>(&text).unwrap(), envelope());
    let unknown = text.replace('{', "{\"kind\":\"cpu\",");
    assert!(
        serde_json::from_str::<Envelope>(&unknown).is_err(),
        "a section with a field Horizon does not read is not an approval"
    );
}

#[test]
fn either_bound_reaching_zero_uses_the_envelope_up() {
    let approved = envelope();
    let inside = approved.remaining(Spent {
        hours: 4.0,
        amount: 1.0,
    });
    assert!(!inside.used_up(), "with hours and amount left it admits a rent");

    let no_hours = approved.remaining(Spent {
        hours: 6.0,
        amount: 0.5,
    });
    assert!(no_hours.used_up(), "hours alone can end an envelope");
    assert_eq!(no_hours.hours, 0.0);

    let no_amount = approved.remaining(Spent {
        hours: 1.0,
        amount: 2.0,
    });
    assert!(no_amount.used_up(), "the amount alone can end an envelope");
    assert_eq!(no_amount.amount, 0.0);

    let over = approved.remaining(Spent {
        hours: 7.0,
        amount: 3.0,
    });
    assert!(over.used_up(), "a rent past the bound leaves nothing");
    assert_eq!((over.hours, over.amount), (0.0, 0.0));
}

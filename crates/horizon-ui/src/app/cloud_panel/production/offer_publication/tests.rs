use super::*;

#[test]
fn each_ready_worker_gets_each_fresh_list_once_and_failures_wait() {
    let now = Instant::now();
    let first = now.checked_sub(Duration::from_mins(20)).unwrap();
    let ready = vec![("a".to_owned(), "w1".to_owned()), ("b".to_owned(), "w2".to_owned())];
    let mut delivered = HashMap::new();
    assert_eq!(due(&delivered, &ready, first, now), Some(&ready[0]));
    let sent = |worker: &str, observed| Delivery {
        worker: worker.into(),
        observed: Some(observed),
        retry_at: None,
        hetzner: None,
        has_catalog: None,
        hetzner_retry_at: None,
        holds_runpod: None,
    };
    delivered.insert("a".to_owned(), sent("w1", first));
    assert_eq!(due(&delivered, &ready, first, now), Some(&ready[1]));
    delivered.insert("b".to_owned(), sent("w2", first));
    assert_eq!(due(&delivered, &ready, first, now), None);
    // A fresh list goes to every worker again.
    assert_eq!(due(&delivered, &ready, now, now), Some(&ready[0]));
    // A worker that failed waits before it is asked again, unless it was replaced.
    delivered.insert(
        "a".to_owned(),
        Delivery {
            retry_at: Some(now + RETRY),
            ..sent("w1", first)
        },
    );
    delivered.insert("b".to_owned(), sent("w2", now));
    assert_eq!(due(&delivered, &ready, now, now), None);
    assert_eq!(due(&delivered, &ready, now, now + RETRY), Some(&ready[0]));
    let replaced = vec![("a".to_owned(), "w3".to_owned())];
    assert_eq!(due(&delivered, &replaced, now, now), Some(&replaced[0]));
    // A worker never tried goes before failed ones whose retry is due, and among
    // those the longest waiting goes first.
    let three = vec![
        ("a".to_owned(), "w1".to_owned()),
        ("b".to_owned(), "w2".to_owned()),
        ("c".to_owned(), "w4".to_owned()),
    ];
    let failed = |retry_at| Delivery {
        worker: String::new(),
        observed: None,
        retry_at: Some(retry_at),
        hetzner: None,
        has_catalog: None,
        hetzner_retry_at: None,
        holds_runpod: None,
    };
    let mut retries = HashMap::new();
    retries.insert(
        "a".to_owned(),
        Delivery {
            worker: "w1".into(),
            ..failed(now + RETRY)
        },
    );
    retries.insert(
        "b".to_owned(),
        Delivery {
            worker: "w2".into(),
            ..failed(now)
        },
    );
    assert_eq!(due(&retries, &three, now, now + RETRY), Some(&three[2]));
    retries.insert(
        "c".to_owned(),
        Delivery {
            worker: "w4".into(),
            ..failed(now + RETRY * 2)
        },
    );
    assert_eq!(due(&retries, &three, now, now + RETRY), Some(&three[1]));
}

type Sent = (String, Option<Snapshot>, Option<HetznerSnapshot>);

/// A state whose sends are recorded, answering the Hetzner part of each with the
/// next of `outcomes`.
fn recording(outcomes: Vec<Hetzner>) -> (State, Receiver<Sent>) {
    let (sent, received) = channel::<Sent>();
    let sent = std::sync::Mutex::new(sent);
    let outcomes = std::sync::Mutex::new(outcomes);
    let state = State {
        publisher: Arc::new(
            move |_: &Path, cloud: &str, snapshot: Option<&Snapshot>, hetzner: Option<&HetznerSnapshot>| {
                sent.lock()
                    .unwrap()
                    .send((cloud.to_owned(), snapshot.cloned(), hetzner.cloned()))
                    .unwrap();
                let outcome = if hetzner.is_some() {
                    outcomes.lock().unwrap().remove(0)
                } else {
                    Hetzner::NotSent
                };
                Ok((Published::Sent, outcome))
            },
        ),
        ..State::default()
    };
    (state, received)
}

fn priced() -> Fetched<(PriceList, Preferences)> {
    let list = PriceList {
        provider: "RunPod",
        cpu: Vec::new(),
        gpus: Vec::new(),
        data_centers: Vec::new(),
        regions: std::collections::BTreeMap::new(),
        storage: horizon_core::cloud_runtime::prices::RUNPOD_STORAGE,
    };
    let preferences = Preferences {
        cpu_flavors: vec!["cpu3c".into()],
        gpu_types: Vec::new(),
    };
    Fetched {
        value: (list, preferences),
        at: Instant::now(),
    }
}

fn catalog(observed_at_millis: u64) -> HetznerSnapshot {
    HetznerSnapshot {
        version: VERSION,
        observed_at_millis,
        catalog: serde_json::from_value(serde_json::json!({
            "offers": [{"server_type": "cx43", "location": "hel1", "cores": 8, "memory_gb": 16.0, "disk_gb": 160,
                "dedicated": false, "hourly_eur": 0.0256, "monthly_eur": 15.99, "available": true, "recommended": true}],
            "volume_gb_month_eur": 0.0572, "ipv4_month_eur": {"hel1": 0.5}, "ipv4_hour_eur": {"hel1": 0.0008},
            "regions": {"hel1": "EUROPE"},
        }))
        .unwrap(),
    }
}

/// Collects the running send as of `now`.
fn finish(state: &mut State, now: Instant) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.job.is_some() {
        assert!(Instant::now() < deadline, "the send finishes");
        state.poll(now);
        std::thread::yield_now();
    }
}

#[test]
fn the_no_binding_catalog_keeps_holders_current_and_a_bound_empty_one_goes_to_all() {
    let (mut state, received) = recording(vec![
        Hetzner::Delivered,
        Hetzner::Delivered,
        Hetzner::Delivered,
        Hetzner::Failed,
    ]);
    let fetched = priced();
    let ctx = egui::Context::default();
    let now = Instant::now();
    let tombstone = HetznerSnapshot {
        catalog: super::empty_catalog(),
        ..catalog(1)
    };
    let known = |has_catalog| Delivery {
        worker: "w1".into(),
        observed: Some(fetched.at),
        retry_at: None,
        hetzner: Some(now.checked_sub(Duration::from_mins(20)).unwrap()),
        has_catalog: Some(has_catalog),
        hetzner_retry_at: None,
        holds_runpod: None,
    };
    let ready = vec![("a".to_owned(), "w1".to_owned())];
    let send = |state: &mut State, ready: &[(String, String)], snapshot: &HetznerSnapshot, at: Instant| {
        state.step(
            Path::new("/unused"),
            ready,
            (Some(&fetched), false),
            Some((snapshot.clone(), at)),
            now,
            &ctx,
        );
    };
    // A worker known to hold no catalog never needs the no-binding one.
    state.delivered.insert("a".to_owned(), known(false));
    send(&mut state, &ready, &tombstone, now);
    assert!(state.job.is_none());
    // A bound catalog goes to it even when location filtering left no offers.
    let mut bound_empty = catalog(2);
    bound_empty.catalog.offers.clear();
    send(&mut state, &ready, &bound_empty, now);
    let (_, snapshot, sent_hetzner) = received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(snapshot.is_none());
    assert_eq!(sent_hetzner, Some(bound_empty));
    finish(&mut state, now);
    assert_eq!(state.delivered["a"].has_catalog, Some(true));
    // A worker holding a catalog gets the no-binding one, and each newer one, so it
    // never goes stale there.
    for at in [Instant::now(), Instant::now() + Duration::from_mins(15)] {
        send(&mut state, &ready, &tombstone, at);
        let (_, snapshot, sent_hetzner) = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(snapshot.is_none());
        assert_eq!(sent_hetzner.as_ref(), Some(&tombstone));
        finish(&mut state, now);
        assert_eq!(state.delivered["a"].hetzner, Some(at));
    }
    // A worker Horizon knows nothing about, as after a restart, is tried once with
    // its list; an older image refusing it is not asked again.
    let unknown = vec![("c".to_owned(), "w3".to_owned())];
    let later = Instant::now() + Duration::from_mins(30);
    send(&mut state, &unknown, &tombstone, later);
    let (cloud, snapshot, sent_hetzner) = received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!((cloud.as_str(), snapshot.is_some()), ("c", true));
    assert_eq!(sent_hetzner.as_ref(), Some(&tombstone));
    finish(&mut state, now);
    assert_eq!(state.delivered["c"].has_catalog, Some(false));
    assert_eq!(state.delivered["c"].hetzner_retry_at, None);
    send(&mut state, &unknown, &tombstone, later);
    assert!(state.job.is_none());
}

#[test]
fn a_ready_worker_receives_the_current_prices_and_is_marked_delivered() {
    let (mut state, received) = recording(vec![Hetzner::Delivered]);
    let fetched = priced();
    let ready = vec![("a".to_owned(), "w1".to_owned())];
    let ctx = egui::Context::default();
    let observed = Instant::now();
    let now = Instant::now();
    state.step(
        Path::new("/unused"),
        &ready,
        (Some(&fetched), false),
        Some((catalog(1), observed)),
        now,
        &ctx,
    );
    let (cloud, snapshot, sent_hetzner) = received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        sent_hetzner,
        Some(catalog(1)),
        "Hetzner's catalog travels with the list"
    );
    assert_eq!(cloud, "a");
    let snapshot = snapshot.unwrap();
    let (list, preferences) = fetched.value.clone();
    assert_eq!(
        (snapshot.version, snapshot.list, snapshot.preferences),
        (VERSION, list, preferences)
    );
    assert!(snapshot.observed_at_millis > 0);
    finish(&mut state, now);
    assert_eq!(state.delivered["a"].observed, Some(fetched.at));
    assert_eq!(state.delivered["a"].hetzner, Some(observed));
    assert!(state.next_check.is_none(), "the next frame checks for more due workers");
    // The worker has this list and catalog now, so nothing more is sent.
    state.step(
        Path::new("/unused"),
        &ready,
        (Some(&fetched), false),
        Some((catalog(1), observed)),
        now,
        &ctx,
    );
    assert!(state.job.is_none());
}

#[test]
fn without_a_runpod_list_workers_get_the_hetzner_catalog_alone() {
    let (mut state, received) = recording(vec![Hetzner::Delivered, Hetzner::Delivered]);
    let ready = vec![("a".to_owned(), "w1".to_owned())];
    let ctx = egui::Context::default();
    let now = Instant::now();
    let at = Instant::now();
    // A worker that has had nothing yet gets the catalog, with no list.
    state.step(
        Path::new("/unused"),
        &ready,
        (None, false),
        Some((catalog(1), at)),
        now,
        &ctx,
    );
    let (_, snapshot, sent) = received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(snapshot.is_none());
    assert_eq!(sent, Some(catalog(1)));
    finish(&mut state, now);
    assert_eq!(state.delivered["a"].hetzner, Some(at));
    assert_eq!(state.delivered["a"].observed, None);
    // Once it has it, nothing more goes until a newer catalog.
    state.step(
        Path::new("/unused"),
        &ready,
        (None, false),
        Some((catalog(1), at)),
        now,
        &ctx,
    );
    assert!(state.job.is_none());
    // Nor does an empty catalog go to a worker that never had one.
    let (mut fresh, _) = recording(vec![Hetzner::Delivered]);
    let empty = HetznerSnapshot {
        version: VERSION,
        observed_at_millis: 1,
        catalog: empty_catalog(),
    };
    fresh.step(
        Path::new("/unused"),
        &ready,
        (None, false),
        Some((empty, at)),
        now,
        &ctx,
    );
    assert!(fresh.job.is_none());
}

#[test]
fn without_a_runpod_key_each_worker_is_told_once_to_drop_its_runpod_prices() {
    let (mut state, received) = recording(vec![Hetzner::Delivered, Hetzner::Delivered]);
    let (cleared, clears) = channel::<String>();
    let cleared = std::sync::Mutex::new(cleared);
    // Worker "b" runs an older image without the command.
    state.clearer = Arc::new(move |_: &Path, cloud: &str| {
        cleared.lock().unwrap().send(cloud.to_owned()).unwrap();
        if cloud == "b" {
            Err("Companion SSH command failed; check connectivity and worker image support".into())
        } else {
            Ok(Published::Sent)
        }
    });
    let mut ready = vec![("a".to_owned(), "w1".to_owned()), ("b".to_owned(), "w2".to_owned())];
    let ctx = egui::Context::default();
    let now = Instant::now();
    let fetched = priced();
    for _ in 0..2 {
        state.step(Path::new("/unused"), &ready, (Some(&fetched), false), None, now, &ctx);
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        finish(&mut state, now);
    }
    assert_eq!(state.delivered["a"].holds_runpod, Some(true));
    // The RunPod key is gone: each worker is told once, before the Hetzner catalog goes.
    let at = Instant::now();
    for cloud in ["a", "b"] {
        state.step(
            Path::new("/unused"),
            &ready,
            (None, true),
            Some((catalog(1), at)),
            now,
            &ctx,
        );
        assert_eq!(clears.recv_timeout(Duration::from_secs(5)).unwrap(), cloud);
        finish(&mut state, now);
        assert_eq!(state.delivered[cloud].holds_runpod, Some(false));
    }
    assert!(received.try_recv().is_err(), "nothing else was sent while clearing");
    // The refusal is not retried; the catalog follows for both.
    for cloud in ["a", "b"] {
        state.step(
            Path::new("/unused"),
            &ready,
            (None, true),
            Some((catalog(1), at)),
            now,
            &ctx,
        );
        let (to, snapshot, sent) = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((to.as_str(), snapshot, sent), (cloud, None, Some(catalog(1))));
        finish(&mut state, now);
    }
    state.step(
        Path::new("/unused"),
        &ready,
        (None, true),
        Some((catalog(1), at)),
        now,
        &ctx,
    );
    assert!(state.job.is_none());
    assert!(clears.try_recv().is_err());
    // A replacement worker may hold prices from before; it is told too.
    ready[0].1 = "w3".into();
    state.step(
        Path::new("/unused"),
        &ready,
        (None, true),
        Some((catalog(1), at)),
        now,
        &ctx,
    );
    assert_eq!(clears.recv_timeout(Duration::from_secs(5)).unwrap(), "a");
    finish(&mut state, now);
    // With a RunPod key, nothing is cleared.
    ready[0].1 = "w4".into();
    state.step(Path::new("/unused"), &ready, (Some(&fetched), false), None, now, &ctx);
    received.recv_timeout(Duration::from_secs(5)).unwrap();
    finish(&mut state, now);
    assert!(clears.try_recv().is_err());
}

#[test]
fn a_newer_catalog_goes_alone_and_a_refused_one_waits_for_its_retry() {
    let (mut state, received) = recording(vec![Hetzner::Delivered, Hetzner::Delivered, Hetzner::Failed]);
    let fetched = priced();
    let ready = vec![("a".to_owned(), "w1".to_owned())];
    let ctx = egui::Context::default();
    let now = Instant::now();
    let first = Instant::now();
    state.step(
        Path::new("/unused"),
        &ready,
        (Some(&fetched), false),
        Some((catalog(1), first)),
        now,
        &ctx,
    );
    received.recv_timeout(Duration::from_secs(5)).unwrap();
    finish(&mut state, now);
    // A newer catalog goes to the worker alone, without the list it already has.
    let second = Instant::now();
    state.step(
        Path::new("/unused"),
        &ready,
        (Some(&fetched), false),
        Some((catalog(2), second)),
        now,
        &ctx,
    );
    let (_, snapshot, sent_hetzner) = received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(snapshot.is_none());
    assert_eq!(sent_hetzner, Some(catalog(2)));
    finish(&mut state, now);
    assert_eq!(state.delivered["a"].hetzner, Some(second));
    // A catalog the worker refuses, as older images do, is asked again only after
    // its retry, while the list stays delivered.
    let third = Instant::now();
    let refused_at = Instant::now();
    state.step(
        Path::new("/unused"),
        &ready,
        (Some(&fetched), false),
        Some((catalog(3), third)),
        refused_at,
        &ctx,
    );
    received.recv_timeout(Duration::from_secs(5)).unwrap();
    finish(&mut state, refused_at);
    let delivery = &state.delivered["a"];
    assert_eq!(delivery.hetzner, Some(second), "a refused catalog is not recorded");
    assert_eq!(delivery.hetzner_retry_at, Some(refused_at + HETZNER_RETRY));
    assert_eq!(delivery.observed, Some(fetched.at));
    state.step(
        Path::new("/unused"),
        &ready,
        (Some(&fetched), false),
        Some((catalog(3), third)),
        refused_at,
        &ctx,
    );
    assert!(state.job.is_none(), "no retry before it is due");
    state.step(
        Path::new("/unused"),
        &ready,
        (Some(&fetched), false),
        Some((catalog(3), third)),
        refused_at + HETZNER_RETRY,
        &ctx,
    );
    let (_, snapshot, _) = received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(snapshot.is_none(), "the retry sends only the catalog");
}

#[test]
fn a_failed_delivery_keeps_the_prices_the_worker_already_has() {
    let now = Instant::now();
    let earlier = now.checked_sub(Duration::from_mins(15)).unwrap();
    let mut state = State::default();
    state.delivered.insert(
        "a".to_owned(),
        Delivery {
            worker: "w1".into(),
            observed: Some(earlier),
            retry_at: None,
            hetzner: None,
            has_catalog: None,
            hetzner_retry_at: None,
            holds_runpod: None,
        },
    );
    let (sender, receiver) = channel();
    state.job = Some(Job {
        cloud: "a".into(),
        worker: "w1".into(),
        observed: Some(now),
        hetzner: None,
        bound: true,
        clear: false,
        receiver,
    });
    sender.send(Err("unreachable".into())).unwrap();
    state.poll(now);
    // A send that ends without an answer is a failure too, never a stuck job.
    let expected = state.delivered["a"].clone();
    let (sender, receiver) = channel();
    drop(sender);
    state.job = Some(Job {
        cloud: "a".into(),
        worker: "w1".into(),
        observed: Some(now),
        hetzner: None,
        bound: true,
        clear: false,
        receiver,
    });
    state.poll(now);
    assert!(state.job.is_none());
    assert_eq!(state.delivered["a"], expected);
    assert!(state.job.is_none());
    assert_eq!(
        state.delivered["a"],
        Delivery {
            worker: "w1".into(),
            observed: Some(earlier),
            retry_at: Some(now + RETRY),
            hetzner: None,
            has_catalog: None,
            hetzner_retry_at: None,
            holds_runpod: None,
        }
    );
}

use super::super::{
    budget::{MAX_STARTS_PER_WINDOW, START_WINDOW},
    decode,
    progress::CREDENTIAL_BUDGET,
};
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc::Sender,
};

fn profile() -> RemoteProviderProfile {
    serde_json::from_str(r#"{"adapter":"browserstack","endpoint":"https://hub-cloud.browserstack.com/wd/hub"}"#)
        .unwrap()
}

fn rebound(profile: &RemoteProviderProfile, seconds: u32) -> RemoteProviderProfile {
    let mut changed = profile.clone();
    changed.limits.max_session_seconds += seconds;
    changed
}

fn rows() -> Vec<CatalogDevice> {
    decode(
        "account",
        br#"[{"os":"ios","os_version":"18","browser":"iphone","device":"iPhone 16","real_mobile":true}]"#,
    )
    .unwrap()
}

fn query() -> CatalogQuery {
    CatalogQuery {
        provider: "account".into(),
        ..Default::default()
    }
}

/// A job that enters `stage`, signals it is there, then waits for its outcome.
fn held(
    stage: CatalogStage,
    starts: &Arc<AtomicUsize>,
) -> (
    Sender<Outcome>,
    Receiver<()>,
    impl FnOnce(&CatalogProgress) -> Outcome + Send + 'static,
) {
    let (release, outcome) = channel();
    let (entered, reached) = channel();
    let starts = Arc::clone(starts);
    let job = move |progress: &CatalogProgress| {
        starts.fetch_add(1, Ordering::SeqCst);
        progress.enter(stage);
        entered.send(()).unwrap();
        outcome.recv().unwrap_or(Err(CatalogError::Unavailable))
    };
    (release, reached, job)
}

/// The number of discovered rows, None while discovery is running.
fn state(cache: &CatalogCache, profile: &RemoteProviderProfile) -> Result<Option<usize>, CatalogError> {
    cache.page(profile, &query()).map(|page| page.map(|page| page.total))
}

fn settle(cache: &mut CatalogCache, done: impl Fn(&CatalogCache) -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        cache.poll();
        if done(cache) {
            return;
        }
        assert!(Instant::now() < end, "catalog job did not settle");
        std::thread::yield_now();
    }
}

#[test]
fn a_healthy_request_is_shared_by_every_consumer() {
    let profile = profile();
    let starts = Arc::new(AtomicUsize::new(0));
    let mut cache = CatalogCache::default();
    let (release, reached, job) = held(CatalogStage::Request, &starts);
    cache.start("account", &profile, job);
    reached.recv().unwrap();
    for _ in 0..10 {
        assert!(!cache.needs_refresh("account", &profile));
        cache.start("account", &profile, |_| unreachable!("a duplicate request started"));
        assert_eq!(state(&cache, &profile), Ok(None));
    }
    // Past the credential budget but within the request stage's own budget.
    cache.advance_clock(CREDENTIAL_BUDGET + Duration::from_secs(1));
    assert_eq!(state(&cache, &profile), Ok(None));
    assert!(
        !cache.needs_refresh("account", &profile),
        "a request within its own budget is never duplicated"
    );
    release.send(Ok(rows())).unwrap();
    settle(&mut cache, |cache| {
        state(cache, &profile).is_ok_and(|page| page.is_some())
    });
    assert_eq!(starts.load(Ordering::SeqCst), 1);
}

#[test]
fn a_blocked_credential_read_is_reported_bounded_and_its_late_result_is_kept() {
    let profile = profile();
    let starts = Arc::new(AtomicUsize::new(0));
    let mut cache = CatalogCache::default();
    let (first, reached, job) = held(CatalogStage::Credentials, &starts);
    cache.start("account", &profile, job);
    reached.recv().unwrap();
    assert_eq!(cache.stage("account"), Some(CatalogStage::Credentials));
    assert_eq!(state(&cache, &profile), Ok(None));

    cache.advance_clock(CREDENTIAL_BUDGET);
    assert_eq!(state(&cache, &profile), Err(CatalogError::CredentialsTimedOut));
    assert!(!cache.needs_refresh("account", &profile), "retries wait for RETRY");

    cache.advance_clock(RETRY.saturating_sub(CREDENTIAL_BUDGET));
    assert!(
        cache.needs_refresh("account", &profile),
        "a stuck read does not block recovery"
    );
    let (second, reached, job) = held(CatalogStage::Credentials, &starts);
    cache.start("account", &profile, job);
    reached.recv().unwrap();
    assert_eq!(
        state(&cache, &profile),
        Ok(None),
        "the healthy replacement answers for this binding"
    );

    cache.advance_clock(RETRY);
    assert_eq!(state(&cache, &profile), Err(CatalogError::CredentialsTimedOut));
    assert!(
        !cache.needs_refresh("account", &profile),
        "two stuck reads exhaust this provider's job budget"
    );
    cache.start("account", &profile, |_| unreachable!("a third concurrent job started"));

    first.send(Ok(rows())).unwrap();
    settle(&mut cache, |cache| {
        state(cache, &profile).is_ok_and(|page| page.is_some())
    });
    assert!(
        cache.target(&profile, &rows()[0].target).is_ok(),
        "a late result for the same binding is useful"
    );
    second.send(Err(CatalogError::Credentials)).unwrap();
    settle(&mut cache, |cache| cache.entries["account"].attempts.is_empty());
    assert!(
        state(&cache, &profile).is_ok_and(|page| page.is_some()),
        "a later failure never replaces fresh rows"
    );
    assert_eq!(starts.load(Ordering::SeqCst), 2);
}

#[test]
fn a_late_answer_beats_a_newer_failure_for_the_same_binding() {
    let profile = profile();
    let starts = Arc::new(AtomicUsize::new(0));
    let mut cache = CatalogCache::default();
    let (stuck, reached, job) = held(CatalogStage::Credentials, &starts);
    cache.start("account", &profile, job);
    reached.recv().unwrap();
    cache.advance_clock(RETRY);
    assert!(cache.needs_refresh("account", &profile));
    cache.start("account", &profile, |_| Err(CatalogError::Unavailable));
    settle(&mut cache, |cache| {
        state(cache, &profile) == Err(CatalogError::Unavailable)
    });
    stuck.send(Ok(rows())).unwrap();
    settle(&mut cache, |cache| state(cache, &profile) == Ok(Some(1)));
}

#[test]
fn slow_requests_and_decoding_stall_separately_from_credentials() {
    let profile = profile();
    let starts = Arc::new(AtomicUsize::new(0));
    for stage in [CatalogStage::Request, CatalogStage::Decode] {
        let mut cache = CatalogCache::default();
        let (release, reached, job) = held(stage, &starts);
        cache.start("account", &profile, job);
        reached.recv().unwrap();
        assert_eq!(cache.stage("account"), Some(stage));
        let mut waited = Duration::ZERO;
        while state(&cache, &profile) == Ok(None) {
            cache.advance_clock(Duration::from_secs(1));
            waited += Duration::from_secs(1);
            assert!(waited <= RETRY, "{stage:?} never stalled");
        }
        assert_eq!(
            state(&cache, &profile),
            Err(CatalogError::Unavailable),
            "{stage:?} is not a credential failure"
        );
        let budget = if stage == CatalogStage::Request {
            Duration::from_secs(12)
        } else {
            Duration::from_secs(5)
        };
        assert_eq!(waited, budget, "{stage:?}");
        release.send(Ok(rows())).unwrap();
        settle(&mut cache, |cache| {
            state(cache, &profile).is_ok_and(|page| page.is_some())
        });
    }
}

#[test]
fn binding_changes_invalidate_at_once_and_starts_stay_bounded() {
    let profile = profile();
    let starts = Arc::new(AtomicUsize::new(0));
    let mut cache = CatalogCache::default();
    let (old, reached, job) = held(CatalogStage::Request, &starts);
    cache.start("account", &profile, job);
    reached.recv().unwrap();

    let second = rebound(&profile, 1);
    assert!(
        cache.needs_refresh("account", &second),
        "a changed binding refreshes at once"
    );
    let (current, reached, job) = held(CatalogStage::Request, &starts);
    cache.start("account", &second, job);
    reached.recv().unwrap();

    let third = rebound(&profile, 2);
    assert!(
        !cache.needs_refresh("account", &third),
        "two running jobs fill the budget"
    );
    cache.start("account", &third, |_| unreachable!("a third concurrent job started"));
    assert_eq!(state(&cache, &third), Ok(None), "healthy jobs will release capacity");
    assert_eq!(state(&cache, &second), Err(CatalogError::RefreshRequired));

    old.send(Ok(rows())).unwrap();
    current.send(Ok(rows())).unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    while cache.budgets["account"].refusal(cache.now()).is_none() {
        assert!(Instant::now() < end, "released jobs did not exit");
        std::thread::yield_now();
    }
    cache.poll();
    for stale in [&profile, &second] {
        assert_eq!(
            cache.target(stale, &rows()[0].target).unwrap_err(),
            CatalogError::RefreshRequired,
            "results from a replaced binding are discarded"
        );
    }

    // Two starts so far; the rest of the window allows quick rebinding only up to the cap.
    let mut binding = 3;
    for _ in 2..MAX_STARTS_PER_WINDOW {
        let profile = rebound(&profile, binding);
        binding += 1;
        let rows = rows();
        assert!(cache.needs_refresh("account", &profile));
        cache.start("account", &profile, move |_| Ok(rows));
        settle(&mut cache, |cache| {
            state(cache, &profile).is_ok_and(|page| page.is_some())
        });
    }
    let limited = rebound(&profile, binding);
    assert!(!cache.needs_refresh("account", &limited));
    cache.start("account", &limited, |_| unreachable!("start rate exceeded"));
    assert_eq!(state(&cache, &limited), Err(CatalogError::Unavailable));

    cache.advance_clock(START_WINDOW);
    assert!(
        cache.needs_refresh("account", &limited),
        "discovery recovers without a restart"
    );
    assert_eq!(starts.load(Ordering::SeqCst), 2);
}

#[test]
fn credential_changes_drop_late_results_but_keep_running_jobs_counted() {
    let profile = profile();
    let starts = Arc::new(AtomicUsize::new(0));
    let mut cache = CatalogCache::default();
    let rows = rows();
    let ready = rows.clone();
    cache.start("account", &profile, move |_| Ok(ready));
    settle(&mut cache, |cache| {
        state(cache, &profile).is_ok_and(|page| page.is_some())
    });
    assert!(cache.target(&profile, &rows[0].target).is_ok());

    let mut releases = Vec::new();
    for generation in 1..=2 {
        cache.invalidate_credentials(generation);
        assert_eq!(
            cache.target(&profile, &rows[0].target).unwrap_err(),
            CatalogError::RefreshRequired
        );
        let (release, reached, job) = held(CatalogStage::Credentials, &starts);
        cache.start("account", &profile, job);
        reached.recv().unwrap();
        releases.push(release);
    }
    cache.invalidate_credentials(3);
    assert!(
        !cache.needs_refresh("account", &profile),
        "jobs from old credentials still count until they exit"
    );
    assert_eq!(state(&cache, &profile), Ok(None));
    cache.advance_clock(CREDENTIAL_BUDGET);
    assert_eq!(
        state(&cache, &profile),
        Err(CatalogError::CredentialsTimedOut),
        "a stuck read from old credentials explains the wait"
    );
    for release in releases {
        release.send(Ok(rows.clone())).unwrap();
    }
    let end = Instant::now() + Duration::from_secs(5);
    while !cache.needs_refresh("account", &profile) {
        assert!(Instant::now() < end, "finished jobs release their budget");
        std::thread::yield_now();
    }
    assert!(
        cache.target(&profile, &rows[0].target).is_err(),
        "old credentials never repopulate the cache"
    );
}

#[test]
fn rows_expire_and_are_never_guessed() {
    let profile = profile();
    let mut cache = CatalogCache::default();
    let rows = rows();
    let ready = rows.clone();
    cache.start("account", &profile, move |_| Ok(ready));
    settle(&mut cache, |cache| {
        state(cache, &profile).is_ok_and(|page| page.is_some())
    });
    assert!(!cache.needs_refresh("account", &profile));
    cache.advance_clock(FRESH);
    assert_eq!(
        cache.target(&profile, &rows[0].target).unwrap_err(),
        CatalogError::RefreshRequired
    );
    assert!(cache.needs_refresh("account", &profile));
    assert_eq!(
        cache.target(&profile, "catalog.account.forged").unwrap_err(),
        CatalogError::RefreshRequired
    );
}

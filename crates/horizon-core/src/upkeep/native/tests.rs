use super::*;

fn config() -> Config {
    Config {
        repositories: vec!["Acme/Web".into(), "acme/api".into()],
        ..Config::default()
    }
}

fn head(c: char) -> String {
    std::iter::repeat_n(c, 40).collect()
}

fn raw(repository: &str, number: u64, head: &str) -> serde_json::Value {
    serde_json::json!({"repository": repository, "pull_request": number, "head": head, "reason": "Avalonia\u{1b}[2J bump"})
}

#[test]
fn only_a_request_for_a_listed_repository_with_a_number_and_a_full_head_is_taken() {
    let requests = requests(
        &[
            raw("acme/web", 7, &head('a')),
            raw("acme/web", 7, &head('a')),
            raw("other/web", 7, &head('a')),
            raw("acme/web", 0, &head('a')),
            raw("acme/web", 8, &head('A')),
            raw("acme/web", 9, "abc"),
            serde_json::json!({"repository": "acme/web", "pull_request": "10", "head": head('a')}),
            serde_json::json!("acme/web"),
        ],
        &config(),
    );
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0].repository, "Acme/Web", "the spelling of the Upkeep");
    assert_eq!(requests[0].reason, "Avalonia[2J bump", "no control characters");
}

#[test]
fn a_request_is_read_once_per_head_and_tests_run_one_at_a_time() {
    let mut ledger = Ledger::default();
    let first = requests(
        &[raw("acme/web", 7, &head('a')), raw("acme/api", 3, &head('b'))],
        &config(),
    );
    assert!(ledger.take(first.clone()));
    assert!(!ledger.take(first.clone()), "the next exchange reads the same files");
    assert_eq!(ledger.start_next().as_ref(), Some(&first[0]));
    assert_eq!(ledger.start_next(), None, "one test cloud at a time");
    assert!(!ledger.take(first[..1].to_vec()), "the test that runs");
    ledger.finish(Report::new(&first[0], State::Passed, "ok"));
    assert!(!ledger.take(first[..1].to_vec()), "a report for that head");
    let moved = requests(&[raw("acme/web", 7, &head('c'))], &config());
    assert!(ledger.take(moved), "a new head of the same pull request");
    assert_eq!(
        ledger.start_next().as_ref(),
        Some(&first[1]),
        "in the order of the requests"
    );
    let states: Vec<_> = ledger
        .reports
        .iter()
        .map(|report| (report.pull_request, report.state))
        .collect();
    assert_eq!(
        states,
        [(3, State::Starting), (7, State::Queued)],
        "one report per pull request, newest first"
    );
}

#[test]
fn the_queue_and_the_reports_stay_bounded() {
    let mut ledger = Ledger::default();
    let many: Vec<_> = (1..=40).map(|n| raw("acme/web", n, &head('a'))).collect();
    for chunk in many.chunks(QUEUE_LIMIT) {
        ledger.take(requests(chunk, &config()));
    }
    assert_eq!(ledger.queue.len(), QUEUE_LIMIT);
    for n in 100..140 {
        ledger.report(Report::new(
            &Request {
                repository: "acme/web".into(),
                number: n,
                head: head('a'),
                reason: String::new(),
            },
            State::Refused,
            "no",
        ));
    }
    assert_eq!(ledger.reports.len(), REPORT_LIMIT);
    assert_eq!(ledger.payload().len(), REPORT_LIMIT);
}

#[test]
fn a_test_expires_at_its_time_limit() {
    let current = Current {
        request: requests(&[raw("acme/web", 7, &head('a'))], &config()).remove(0),
        cloud: None,
        started_at: 1_000,
        prompted: false,
    };
    let limit = i64::try_from(TIME_LIMIT.as_secs()).unwrap();
    assert!(!current.expired(1_000 + limit - 1));
    assert!(current.expired(1_000 + limit));
}

#[test]
fn the_task_names_the_pull_request_and_the_evidence_folder() {
    let request = requests(&[raw("acme/web", 7, &head('a'))], &config()).remove(0);
    let task = task(&request);
    assert!(task.contains("#7 of Acme/Web"));
    assert!(task.contains(&head('a')));
    assert!(task.contains(EVIDENCE));
    assert!(!task.contains('\n'), "one pasted message");
}

#[test]
fn the_longest_reports_fit_one_delivery() {
    let mut ledger = Ledger::default();
    for number in 1..=REPORT_LIMIT as u64 * 2 {
        let request = Request {
            repository: format!("{}/{}", "o".repeat(39), "n".repeat(100)),
            number: NUMBER_LIMIT - number,
            head: head('a'),
            reason: String::new(),
        };
        // Quotes are the characters that escaping makes longest.
        let mut report = Report::new(&request, State::Failed, "\"".repeat(MESSAGE_LIMIT * 2));
        report.report = "\"".repeat(REPORT_TEXT_LIMIT);
        report.files = (0..12).map(|n| format!("{n:0>80}")).collect();
        ledger.report(report);
    }
    assert!(
        ledger
            .reports
            .iter()
            .all(|report| report.message.chars().count() == MESSAGE_LIMIT)
    );
    let command = worker::delivery_command(&ledger.payload()).unwrap();
    assert!(command.len() < 100 * 1024, "{}", command.len());
}

#[test]
fn a_test_without_a_result_of_its_agent_starts_again_after_a_while() {
    let request = requests(&[raw("acme/web", 7, &head('a'))], &config()).remove(0);
    for (state, again) in [
        (State::Refused, true),
        (State::Blocked, true),
        (State::Expired, true),
        (State::Failed, false),
        (State::Passed, false),
    ] {
        let mut ledger = Ledger::default();
        ledger.finish(Report::new(&request, state, "ended"));
        assert!(!ledger.take(vec![request.clone()]), "{state:?} just now");
        ledger.tested[0].updated_at -= i64::try_from(RETRY_AFTER.as_secs()).unwrap();
        assert_eq!(ledger.take(vec![request.clone()]), again, "{state:?} an hour ago");
    }
}

#[test]
fn a_tested_head_is_never_taken_again_after_its_report_made_way() {
    let mut ledger = Ledger::default();
    let first = requests(&[raw("acme/web", 1, &head('a'))], &config()).remove(0);
    ledger.finish(Report::new(&first, State::Passed, "ok"));
    for number in 2..40 {
        let request = Request {
            number,
            ..first.clone()
        };
        ledger.finish(Report::new(&request, State::Failed, "no"));
    }
    assert!(ledger.reports.iter().all(|report| report.pull_request != 1));
    let attempts = ledger
        .tested
        .iter()
        .find(|tested| tested.pull_request == 1)
        .unwrap()
        .attempts;
    assert!(ledger.take(vec![first.clone()]), "the agent gets the outcome back");
    assert!(ledger.queue.is_empty(), "no second test of a head that passed");
    assert_eq!(
        (ledger.reports[0].pull_request, ledger.reports[0].state),
        (1, State::Passed)
    );
    assert_eq!(
        ledger
            .tested
            .iter()
            .find(|tested| tested.pull_request == 1)
            .unwrap()
            .attempts,
        attempts,
        "a report given back is not a new test"
    );
    assert!(!ledger.take(vec![first.clone()]), "asked again, nothing changes");
    let newer = Request {
        head: head('b'),
        ..first.clone()
    };
    assert!(ledger.take(vec![newer]));
    assert!(
        !ledger.take(vec![first]),
        "an older head does not take the place of the newer head's report"
    );
    assert_eq!(ledger.reports[0].head, head('b'));
}

#[test]
fn the_reports_of_tests_that_wait_or_run_always_stay() {
    let mut ledger = Ledger::default();
    let waiting: Vec<_> = (1..=QUEUE_LIMIT as u64)
        .map(|n| raw("acme/web", n, &head('a')))
        .collect();
    ledger.take(requests(&waiting, &config()));
    let running = ledger.start_next().unwrap();
    ledger.take(requests(&[raw("acme/web", 99, &head('b'))], &config()));
    for number in 100..140 {
        let request = Request {
            number,
            ..running.clone()
        };
        ledger.report(Report::new(&request, State::Refused, "no"));
    }
    assert_eq!(ledger.reports.len(), REPORT_LIMIT);
    for request in ledger.queue.iter().chain([&running]) {
        assert!(
            ledger
                .reports
                .iter()
                .any(|report| report.pull_request == request.number),
            "{}",
            request.number
        );
    }
}

#[test]
fn a_head_gets_at_most_two_tests() {
    let request = requests(&[raw("acme/web", 7, &head('a'))], &config()).remove(0);
    let mut ledger = Ledger::default();
    let hour = i64::try_from(RETRY_AFTER.as_secs()).unwrap();
    ledger.finish(Report::new(&request, State::Blocked, "no login"));
    ledger.tested[0].updated_at -= hour;
    assert!(ledger.take(vec![request.clone()]), "a second test");
    ledger.queue.clear();
    ledger.finish(Report::new(&request, State::Blocked, "no login"));
    ledger.tested[0].updated_at -= hour;
    assert_eq!(ledger.tested[0].attempts, 2);
    assert!(!ledger.take(vec![request]), "never a third");
}

#[test]
fn refused_tests_make_way_before_a_test_that_rented_a_cloud() {
    let mut ledger = Ledger::default();
    let rented = requests(&[raw("acme/web", 1, &head('a'))], &config()).remove(0);
    ledger.finish(Report::new(&rented, State::Expired, "time limit"));
    for number in 2..(2 + 2 * TESTED_LIMIT as u64) {
        let refused = Request {
            number,
            ..rented.clone()
        };
        ledger.finish(Report::new(&refused, State::Refused, "not open"));
    }
    assert_eq!(ledger.tested.len(), TESTED_LIMIT);
    assert!(
        ledger.tested.iter().any(|tested| tested.pull_request == 1),
        "refused requests cannot push out a head that rented a cloud"
    );
}

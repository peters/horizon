use super::*;
use crate::azure::{
    RETRY_AFTER_CAP,
    operation::{WAIT_SLICE, operation_ref, remaining_at},
    retry_after,
};
use std::time::Instant;

fn status(value: &str) -> String {
    json!({ "status": value }).to_string()
}

/// A submitted deployment's operation, which the test then polls.
fn operation_of(client: &Scripted, cancel: &Cancellation) -> OperationRef {
    match submit(client, cancel).unwrap().operation {
        LongRunning::Accepted(operation) => operation,
        LongRunning::Completed => panic!("the deployment was accepted"),
    }
}

fn seconds(values: &[f64]) -> Vec<Duration> {
    values.iter().copied().map(Duration::from_secs_f64).collect()
}

/// The slices a poll wait of each delay is split into, in order.
fn sliced(values: &[f64]) -> Vec<Duration> {
    let mut slices = Vec::new();
    for delay in seconds(values) {
        let mut left = delay;
        while !left.is_zero() {
            let slice = left.min(WAIT_SLICE);
            slices.push(slice);
            left = left.saturating_sub(slice);
        }
    }
    slices
}

#[test]
fn an_operation_url_outside_the_subscription_is_refused() {
    let escapes = [
        format!("https://management.azure.com/subscriptions/{OTHER_SUB}/operations/op"),
        format!("https://management.azure.com/subscriptions/{SUB}/../../subscriptions/{OTHER_SUB}/operations/op"),
        format!("https://management.azure.com/subscriptions/{SUB}/%2e%2e/{OTHER_SUB}/operations/op"),
        format!("https://management.azure.com/subscriptions/{SUB}/operations\\..\\{OTHER_SUB}/op"),
        format!("https://management.azure.com/subscriptions/{SUB}//operations/op"),
        format!("https://management.azure.com/subscriptions/{SUB}/operations/op?x=/../{OTHER_SUB}"),
        format!("https://management.azure.com/subscriptions/{SUB}/operations/op#fragment"),
        format!("https://management.azure.com/subscriptions/{SUB}/operations/op@example.com/x"),
        format!("https://management.azure.com.example.com/subscriptions/{SUB}/operations/op"),
        format!("http://management.azure.com/subscriptions/{SUB}/operations/op"),
        format!("HTTPS://management.azure.com/subscriptions/{SUB}/operations/op"),
        format!(
            "https://management.azure.com/subscriptions/{SUB}/operations/{}",
            "a".repeat(8_192)
        ),
    ];
    let mut script: Vec<Exchange> = escapes.iter().map(|url| accepted(url, None)).collect();
    script.push(
        put(
            deployment_url("worker-deployment"),
            deployment_request(),
            201,
            deployment_body("worker-deployment", "Accepted"),
        )
        .header("location", operation_url("deploy-1")),
    );
    let client = scripted(script);
    let cancel = Cancellation::default();
    for url in &escapes {
        let failure = submit(&client, &cancel).unwrap_err();
        assert!(!failure.definite(), "ARM accepted the deployment: {url}");
        assert!(matches!(failure.into(), CloudError::InvalidResponse), "{url}");
        let forged = operation_ref(
            &[("azure-asyncoperation", url.as_str())]
                .into_iter()
                .map(|(name, value)| (ureq::http::HeaderName::from_static(name), value.parse().unwrap()))
                .collect(),
        )
        .unwrap();
        assert!(matches!(
            client.azure.await_operation(&forged, &cancel),
            Err(CloudError::InvalidResponse)
        ));
    }
    assert!(
        matches!(
            submit(&client, &cancel).unwrap_err().into(),
            CloudError::InvalidResponse
        ),
        "a Location header alone is another protocol"
    );
    assert!(
        client.sent().iter().all(|line| line.starts_with("PUT ")),
        "no foreign URL was polled"
    );
    assert_eq!(client.sleeps(), Vec::<Duration>::new());
    client.done();
}

#[test]
fn an_operation_is_polled_until_it_succeeds() {
    // ARM signs an operation URL with a certificate of about 3 KB in `c=`.
    let certificate = "MIIHlTCCBn2gAwIBAgIRAKGW41M2RjI0CVOmoqSB-m4_".repeat(72);
    let operation = format!(
        "{}&p=628ba44e-30c2-4d77-8ae4-98243359e7d1&t=639247605279933261&c={certificate}&s=bddvIFxYRvg_1lwZ&h=X-yOGgLsaSE9H9QW",
        operation_url("7bfb09fc-d6d3-4e6a-893f-a5d7501ed7e0")
    );
    assert!(operation.len() > 3_000);
    let client = scripted(vec![
        accepted(&operation, None),
        get(operation.clone(), 200, status("InProgress")),
        get(operation.clone(), 200, status("Running")),
        get(operation.clone(), 200, status("Succeeded")),
    ]);
    let cancel = Cancellation::default();
    let reference = operation_of(&client, &cancel);
    client.azure.await_operation(&reference, &cancel).unwrap();
    assert_eq!(client.sleeps(), sliced(&[0.5, 1.0, 2.0]));
    assert_eq!(client.sent().len(), 4);
    client.done();
}

#[test]
fn a_throttled_poll_waits_retry_after_and_continues() {
    let operation = operation_url("deploy-1");
    let client = scripted(vec![
        accepted(&operation, Some("7")),
        get(operation.clone(), 200, status("InProgress")),
        get(operation.clone(), 429, arm_error("TooManyRequests", "slow down")).header("retry-after", "3"),
        get(operation.clone(), 429, "").header("retry-after", "40"),
        get(operation.clone(), 200, status("InProgress")).header("retry-after", "1"),
        get(operation, 200, status("Succeeded")),
    ]);
    let cancel = Cancellation::default();
    let reference = operation_of(&client, &cancel);
    client.azure.await_operation(&reference, &cancel).unwrap();
    assert_eq!(
        client.sleeps(),
        sliced(&[7.0, 1.0, 3.0, 40.0, 8.0]),
        "Retry-After makes a step longer and never shorter"
    );
    client.done();
    let headers = |value: &str| {
        let mut map = HeaderMap::new();
        map.insert("retry-after", value.parse().unwrap());
        map
    };
    assert_eq!(retry_after(&headers(" 12 ")), Some(Duration::from_secs(12)));
    assert_eq!(
        retry_after(&headers("600")),
        Some(RETRY_AFTER_CAP),
        "an untrusted answer cannot ask for an unbounded wait"
    );
    assert_eq!(retry_after(&headers("Wed, 21 Oct 2026 07:28:00 GMT")), None);
    assert_eq!(retry_after(&headers("-1")), None);
    assert_eq!(retry_after(&HeaderMap::new()), None);
    assert_eq!(operation_ref(&headers("5")), None, "no operation header");
}

#[test]
fn a_cancel_during_a_retry_after_wait_is_seen_within_one_slice() {
    let operation = operation_url("deploy-1");
    let cancel = Cancellation::default();
    // ARM asks for ten minutes. The client caps it, and one slice of the wait is
    // enough for the cancel to be seen.
    let client = scripted_cancelling(vec![accepted(&operation, Some("600"))], cancel.clone());
    let reference = operation_of(&client, &cancel);
    let ended = client.azure.await_operation(&reference, &cancel);
    assert!(matches!(ended, Err(CloudError::Cancelled)), "{ended:?}");
    assert_eq!(client.sleeps(), vec![WAIT_SLICE], "one slice, not ten minutes");
    assert_eq!(client.sent().len(), 1, "the cancelled poll sends nothing");
}

#[test]
fn a_poll_past_its_bound_times_out() {
    // 30.5 s of rising steps, then 30 s steps until ten minutes have passed.
    let polls = 24;
    let operation = operation_url("deploy-1");
    let mut script = vec![accepted(&operation, None)];
    script.extend((0..polls).map(|_| get(operation.clone(), 200, status("InProgress"))));
    let client = scripted(script);
    let cancel = Cancellation::default();
    let reference = operation_of(&client, &cancel);
    let ended = client.azure.await_operation(&reference, &cancel);
    assert!(matches!(
        ended,
        Err(CloudError::Invalid(
            "Provider operation is still running; check again later"
        ))
    ));
    let sleeps = client.sleeps();
    assert_eq!(
        sleeps.iter().sum::<Duration>(),
        ACTION_TIMEOUT,
        "the waits fill the bound exactly"
    );
    assert!(
        sleeps.iter().all(|slice| *slice <= WAIT_SLICE),
        "every wait is sliced, so a cancel is seen inside it"
    );
    assert_eq!(
        &sleeps[..sliced(&[0.5, 1.0, 2.0]).len()],
        sliced(&[0.5, 1.0, 2.0]),
        "the first steps keep their full length in slices"
    );
    assert_eq!(
        client.sent().len(),
        polls + 1,
        "the last wait reaches the bound and sends no poll"
    );
    client.done();
    let now = Instant::now();
    assert_eq!(
        remaining_at(now, now + Duration::from_secs(3)),
        Some(Duration::from_secs(3))
    );
    assert_eq!(remaining_at(now, now), None);
    assert_eq!(remaining_at(now + Duration::from_secs(1), now), None);
}

#[test]
fn a_failed_or_canceled_operation_is_a_refusal_with_its_reason() {
    let operation = operation_url("deploy-1");
    let failed =
        json!({ "status": "Failed", "error": { "code": "DeploymentFailed", "message": "the VM did not start" } });
    let echoed = json!({ "status": "Failed", "error": { "code": "X", "message": format!("bad {TOKEN}") } });
    let client = scripted(vec![
        accepted(&operation, None),
        get(operation.clone(), 200, failed.to_string()),
        get(operation.clone(), 200, status("Canceled")),
        get(operation.clone(), 200, echoed.to_string()),
        get(operation.clone(), 404, ""),
        get(operation.clone(), 200, "{}"),
        get(operation, 500, arm_error("InternalServerError", "boom")),
    ]);
    let cancel = Cancellation::default();
    let reference = operation_of(&client, &cancel);
    let poll = || client.azure.await_operation(&reference, &cancel).unwrap_err();
    let refusal = poll();
    assert!(matches!(refusal, CloudError::Rejected(_)));
    assert_eq!(
        refusal.to_string(),
        "Provider rejected the request or capacity is unavailable: the VM did not start"
    );
    assert_eq!(
        poll().to_string(),
        "Provider rejected the request or capacity is unavailable",
        "a canceled operation without a reason"
    );
    assert!(!poll().to_string().contains("synthetic-arm"));
    assert!(matches!(poll(), CloudError::InvalidResponse), "a 404 operation");
    assert!(matches!(poll(), CloudError::InvalidResponse), "no status");
    assert!(matches!(poll(), CloudError::Transport));
    client.done();
}

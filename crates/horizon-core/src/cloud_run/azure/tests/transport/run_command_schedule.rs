//! The run-command poll schedule as it runs in production: the backoff table, ARM's
//! `Retry-After` lengthening a step (never shortening it), the cap on one wait, and
//! nothing slept once the operation is terminal.
use super::*;

fn accepted() -> Expectation {
    with_header(
        expect("POST", vm_url("/runCommand"), 202, "", Some(run_command_body())),
        "azure-asyncoperation",
        operation_url(),
    )
}

fn poll(status: u16, body: &str, retry_after: Option<&str>) -> Expectation {
    let poll = expect("GET", operation_url(), status, body, None);
    match retry_after {
        Some(seconds) => with_header(poll, "retry-after", seconds.into()),
        None => poll,
    }
}

const IN_PROGRESS: &str = r#"{"status":"InProgress"}"#;
const DONE: &str = r#"{"status":"Succeeded","properties":{"output":{"value":[{"code":"ProvisioningState/succeeded","message":"[stdout]\nok\n[stderr]\n"}]}}}"#;

#[test]
fn polls_follow_the_backoff_table_and_retry_after_only_lengthens_a_capped_step() {
    let (transport, calls, sleeps) = http_with_sleeps(vec![
        accepted(),
        // A far-future Retry-After is honoured only up to the cap.
        poll(200, IN_PROGRESS, Some("3600")),
        // Longer than the table step: the header wins.
        poll(429, "throttled", Some("7")),
        // Malformed guidance falls back to the table.
        poll(200, IN_PROGRESS, Some("soon")),
        // Shorter than the table step: the table wins, a header never shortens a wait.
        poll(200, IN_PROGRESS, Some("0")),
        // The HTTP-date form is not used by ARM's operation resources and is ignored.
        poll(200, IN_PROGRESS, Some("Wed, 21 Oct 2026 07:28:00 GMT")),
        poll(200, DONE, None),
    ]);
    assert_eq!(
        transport.run_command(GROUP, "worker", AzureRunCommand::HostKey),
        Ok(Some("ok".into()))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 7);
    assert_eq!(
        *sleeps.lock().expect("sleeps"),
        [
            Duration::from_millis(500),
            Duration::from_secs(60),
            Duration::from_secs(7),
            Duration::from_secs(4),
            Duration::from_secs(8),
            Duration::from_secs(15),
        ],
        "one wait before each poll, none after the terminal answer"
    );
}

#[test]
fn a_synchronous_answer_and_a_refused_operation_never_wait() {
    let (transport, _, sleeps) = http_with_sleeps(vec![
        expect(
            "POST",
            vm_url("/runCommand"),
            200,
            r#"{"value":[{"code":"ProvisioningState/succeeded","message":"[stdout]\nnow\n[stderr]\n"}]}"#,
            Some(run_command_body()),
        ),
        with_header(
            expect("POST", vm_url("/runCommand"), 202, "", Some(run_command_body())),
            "azure-asyncoperation",
            format!("https://management.azure.com/subscriptions/{FOREIGN_SUB}/operations/op-2"),
        ),
    ]);
    assert_eq!(
        transport.run_command(GROUP, "worker", AzureRunCommand::HostKey),
        Ok(Some("now".into()))
    );
    assert_eq!(
        transport.run_command(GROUP, "worker", AzureRunCommand::HostKey),
        Err(AzureError::InvalidResponse {
            operation: "virtual machine run command"
        })
    );
    assert!(sleeps.lock().expect("sleeps").is_empty(), "no poll, no wait");
}

#[test]
fn the_bounded_poll_sleeps_the_whole_table_exactly_once() {
    let (transport, _, sleeps) = http_with_sleeps(
        std::iter::once(accepted())
            .chain((0..9).map(|_| poll(200, IN_PROGRESS, None)))
            .collect(),
    );
    assert_eq!(
        transport.run_command(GROUP, "worker", AzureRunCommand::HostKey),
        Err(AzureError::OperationTimedOut {
            operation: "virtual machine run command"
        })
    );
    let table: Vec<Duration> = [500, 1_000, 2_000, 4_000, 8_000, 15_000, 30_000, 30_000, 30_000]
        .into_iter()
        .map(Duration::from_millis)
        .collect();
    assert_eq!(*sleeps.lock().expect("sleeps"), table);
}

//! Live acceptance run of the Azure worker adapter against a real subscription.
//!
//! Ignored by default and driven entirely by environment variables, so the ordinary
//! test matrix never touches Azure. It creates exactly one task-owned resource group,
//! proves readiness with the attested host key by opening a pinned SSH session,
//! stops the worker, verifies the retained state, starts its compute again and
//! reattaches through the same pinned key to the retained data, stops it again,
//! deletes the group and proves it is gone. Every step is timed and written as one JSON line per event to stdout.
//!
//! Required: `HORIZON_AZURE_LIVE_SUBSCRIPTION`, `HORIZON_AZURE_LIVE_IMAGE` (digest
//! reference on the registry), `HORIZON_AZURE_LIVE_PULL_IDENTITY` (resource ID),
//! `HORIZON_AZURE_LIVE_REGISTRY` (login server). Optional:
//! `HORIZON_AZURE_LIVE_LOCATION` (default `northeurope`), `HORIZON_AZURE_LIVE_VM_SIZE`
//! (default `Standard_D2s_v3`), `HORIZON_AZURE_LIVE_REAPER_HOURS` (default `2`, 1 to 24,
//! checked before anything is created), `HORIZON_AZURE_LIVE_HOURLY_COST_MICROS`
//! (default `120000`, the declared hourly price of the chosen size; change it together
//! with the size), `HORIZON_AZURE_LIVE_KEEP` (skip deletion and the failure cleanup),
//! `HORIZON_AZURE_LIVE_REAPER_GROUP` and
//! `HORIZON_AZURE_LIVE_REAPER_ACCOUNT` (default `horizon-worker-registry` and
//! `horizon-spike-reaper`).
//!
//! Compute bound without this controller: before anything is created, the run
//! checks that the subscription-side spike reaper schedule is enabled, runs every 15
//! minutes and is due within the next interval; once the worker VM exists (about 15 s after creation) it is tagged for that
//! reaper. The window between deployment submission and the tag is not covered by
//! the reaper: a controller lost in that window leaves an untagged VM that the
//! operator must delete by hand (the adapter's own template does not carry
//! operator tags). A phase that fails after creation requests deletion of the exact
//! group on the way out, unless `HORIZON_AZURE_LIVE_KEEP` is set.
//!
//! Run with `cargo test -p horizon-core --test azure_live_worker -- --ignored --nocapture`.
//!
//! Unix only: the driver relies on `ssh`, process groups and `/bin/kill` to bound
//! every external command including its descendants.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use horizon_core::cloud_run::{
    CloudJobId, CloudProvider, CloudWorkflowId, WorkerLifetime, WorkerTarget,
    azure::{
        AzureCliCredential, AzureClient, AzureDiskSku, AzureError, AzureProfile, WORKER_VM_NAME,
        deployment::{TAG_JOB, TAG_WORKFLOW},
        resource_group_name,
    },
    interactive_worker::{
        InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerLifecycle as Lifecycle,
        InteractiveWorkerProvider, InteractiveWorkerRequest, InteractiveWorkerSshEndpoint, InteractiveWorkerStatus,
    },
    interactive_worker_start::{InteractiveWorkerStart, InteractiveWorkerStartProvider},
    interactive_worker_stop::{
        InteractiveWorkerStop, InteractiveWorkerStopExpectation, InteractiveWorkerStopObservation,
        InteractiveWorkerStopObserver, InteractiveWorkerStopProvider,
    },
};
use std::{
    path::Path,
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const READY_BOUND: Duration = Duration::from_mins(15);
const GONE_BOUND: Duration = Duration::from_mins(20);
const POLL: Duration = Duration::from_secs(10);

struct Run {
    started: Instant,
}

impl Run {
    fn event(&self, name: &str, detail: &str) {
        println!(
            r#"{{"event":"{name}","t_seconds":{:.1},"detail":{}}}"#,
            self.started.elapsed().as_secs_f64(),
            serde_json::Value::String(detail.to_string())
        );
    }
}

/// Run `az` with the given arguments, bounded: the child is killed once `deadline`
/// passes, so a hung CLI can never leave the run (and its billed VM) waiting.
fn az(args: &[&str], deadline: Instant) -> Option<std::process::Output> {
    let mut command = Command::new("az");
    command.args(args);
    bounded(command, deadline)
}

/// Run any command to completion within `deadline`. The command runs in its own
/// process group (Unix), both pipes are drained on threads with a cap, and every wait
/// (the child, then each drained stream) is bounded by the same deadline; on expiry
/// the whole group is killed, so a descendant that inherited a pipe cannot extend the
/// step either.
fn bounded(mut command: Command, deadline: Instant) -> Option<std::process::Output> {
    use std::io::Read as _;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let kill_tree = |child: &mut std::process::Child| {
        #[cfg(unix)]
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .status();
        let _ = child.kill();
        let _ = child.wait();
    };
    // Read each stream to end so the child never sees a closed pipe; keep only the
    // capped prefix; report through a channel so the wait can be bounded.
    let drain = |stream: Option<Box<dyn std::io::Read + Send>>| {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut retained = Vec::new();
            let mut chunk = [0_u8; 8 * 1024];
            if let Some(mut stream) = stream {
                while let Ok(count) = stream.read(&mut chunk) {
                    if count == 0 {
                        break;
                    }
                    let room = OUTPUT_CAP.saturating_sub(retained.len());
                    retained.extend_from_slice(&chunk[..count.min(room)]);
                }
            }
            let _ = sender.send(retained);
        });
        receiver
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
    );
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(200).min(deadline.saturating_duration_since(Instant::now())));
            }
            _ => {
                kill_tree(&mut child);
                return None;
            }
        }
    };
    let remaining = || deadline.saturating_duration_since(Instant::now());
    let (Ok(stdout), Ok(stderr)) = (stdout.recv_timeout(remaining()), stderr.recv_timeout(remaining())) else {
        // A descendant still holds a pipe past the deadline: end the whole group.
        kill_tree(&mut child);
        return None;
    };
    Some(std::process::Output { status, stdout, stderr })
}

/// Longest output kept from one external command.
const OUTPUT_CAP: usize = 4 * 1024 * 1024;

const CLI_STEP: Duration = Duration::from_secs(90);

/// `HORIZON_AZURE_LIVE_KEEP`: leave the worker in place at the end and after a failure.
fn keep() -> bool {
    std::env::var_os("HORIZON_AZURE_LIVE_KEEP").is_some()
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set for the live run"))
}

fn optional(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn generate_client_key(directory: &Path) -> (String, std::path::PathBuf) {
    let private = directory.join("client_ed25519");
    let status = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "", "-f"])
        .arg(&private)
        .status()
        .expect("ssh-keygen");
    assert!(status.success(), "ssh-keygen failed");
    let public = std::fs::read_to_string(private.with_extension("pub")).expect("public key");
    let mut fields = public.split_ascii_whitespace();
    let key = format!("{} {}", fields.next().unwrap(), fields.next().unwrap());
    (key, private)
}

/// The spike reaper deallocates any VM carrying these tags once the deadline passed;
/// merged in so the worker's own identity tags stay intact. Every CLI attempt is
/// bounded; if the tag cannot be installed within the bound the run fails and the
/// armed cleanup guard requests the ownership-checked deletion of the group.
fn arm_reaper(subscription: &str, group: &str, deadline: &str, run: &Run) {
    let vm_id = format!(
        "/subscriptions/{subscription}/resourceGroups/{group}/providers/Microsoft.Compute/virtualMachines/{WORKER_VM_NAME}"
    );
    let until = Instant::now() + READY_BOUND;
    while Instant::now() < until {
        let tag = format!("deadline={deadline}");
        let args = [
            "tag",
            "update",
            "--subscription",
            subscription,
            "--resource-id",
            &vm_id,
            "--operation",
            "merge",
            "--tags",
            "purpose=horizon-azure-vm-spike",
            &tag,
        ];
        if az(&args, (Instant::now() + CLI_STEP).min(until)).is_some_and(|output| output.status.success()) {
            run.event("reaper_armed", &format!("deadline {deadline}"));
            return;
        }
        std::thread::sleep(POLL.min(until.saturating_duration_since(Instant::now())));
    }
    // The armed cleanup guard performs the ownership-checked deletion on unwind.
    panic!("the worker VM never became taggable for the reaper");
}

fn ssh_proof(
    endpoint: &horizon_core::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint,
    private: &Path,
    directory: &Path,
    command: &str,
) -> String {
    let known_hosts = directory.join("known_hosts");
    std::fs::write(
        &known_hosts,
        format!("[{}]:{} {}\n", endpoint.host, endpoint.port, endpoint.host_key),
    )
    .expect("known_hosts");
    let mut ssh = Command::new("ssh");
    // No operator configuration: only the options given here decide which endpoint is
    // contacted and which host key is trusted.
    ssh.args(["-F", "/dev/null"])
        .arg("-i")
        .arg(private)
        .args(["-p", &endpoint.port.to_string()])
        .arg("-o")
        .arg(format!("UserKnownHostsFile={}", known_hosts.display()))
        .args([
            "-o",
            "GlobalKnownHostsFile=/dev/null",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=20",
            "-o",
            "IdentitiesOnly=yes",
        ])
        .arg(format!("{}@{}", endpoint.username, endpoint.host))
        .arg(command);
    // Bounded like every other external step: a stalled session is killed, so the
    // cleanup guard always gets to run.
    let output = bounded(ssh, Instant::now() + CLI_STEP).expect("ssh finished within its bound");
    assert!(
        output.status.success(),
        "pinned ssh failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Poll until `poll` answers, within `bound`: the deadline is checked before every
/// poll and the sleep never crosses it. One poll may itself take up to the adapter's
/// own operation bound (two minutes for a run command), so the wait can end at most
/// that much after `bound`; that is the adapter's bound, not an unbounded wait.
fn wait_for<T>(bound: Duration, mut poll: impl FnMut() -> Result<Option<T>, AzureError>) -> Result<T, String> {
    let until = Instant::now() + bound;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("bound exceeded".into());
        }
        match poll() {
            Ok(Some(value)) => return Ok(value),
            Ok(None) => {}
            Err(error) => return Err(format!("{error:?}")),
        }
        std::thread::sleep(POLL.min(until.saturating_duration_since(Instant::now())));
    }
}

/// One `az` invocation: arguments in, bounded output out. Injected so the cleanup
/// guard's decisions can be tested without Azure.
type Runner = Box<dyn Fn(&[&str], Instant) -> Option<std::process::Output> + Send>;

/// What the failure cleanup did, printed on the way out and asserted in tests.
#[derive(Debug, PartialEq, Eq)]
enum CleanupOutcome {
    /// The group carried this run's identifiers and its deletion was accepted.
    DeletionRequested,
    /// The group is ours but no deletion request was accepted: operator action needed.
    DeletionNotAccepted,
    /// The group is absent, or a same-named group that is not ours: left untouched.
    NotOurs,
    /// Ownership could not be checked at all: operator action needed.
    OwnershipUnknown,
    /// Nothing to do: not armed, or the run asked to keep the worker.
    Skipped,
}

/// Requests deletion of the exact group if the run ends before its own delete step,
/// so a failed phase leaves nothing billing beyond the deletion itself. Disarmed by
/// the delete step and by an explicit keep. Ownership is proven first: the group must
/// carry this run's workflow and job identifiers in the adapter's identity tags, so a
/// same-named group that is not ours is left untouched and reported instead.
struct Cleanup {
    subscription: String,
    group: String,
    workflow_id: String,
    job_id: String,
    armed: bool,
    keep: bool,
    run_az: Runner,
}

impl Cleanup {
    fn live(subscription: String, group: String, workflow_id: String, job_id: String) -> Self {
        Self {
            subscription,
            group,
            workflow_id,
            job_id,
            armed: false,
            keep: keep(),
            run_az: Box::new(az),
        }
    }

    fn owned(&self) -> Option<bool> {
        let shown = (self.run_az)(
            &[
                "group",
                "show",
                "--subscription",
                &self.subscription,
                "--name",
                &self.group,
                "-o",
                "json",
            ],
            Instant::now() + CLI_STEP,
        )?;
        if !shown.status.success() {
            // `az group show` on a missing group exits non-zero: nothing to clean.
            return String::from_utf8_lossy(&shown.stderr)
                .contains("ResourceGroupNotFound")
                .then_some(false);
        }
        let value: serde_json::Value = serde_json::from_slice(&shown.stdout).ok()?;
        let tag = |name: &str| value["tags"][name].as_str().map(str::to_string);
        Some(tag(TAG_WORKFLOW) == Some(self.workflow_id.clone()) && tag(TAG_JOB) == Some(self.job_id.clone()))
    }

    fn request_deletion(&self) -> bool {
        (0..3).any(|_| {
            (self.run_az)(
                &[
                    "group",
                    "delete",
                    "--subscription",
                    &self.subscription,
                    "--name",
                    &self.group,
                    "--yes",
                    "--no-wait",
                ],
                Instant::now() + CLI_STEP,
            )
            .is_some_and(|output| output.status.success())
        })
    }

    /// Decide and act once; `Drop` calls this and prints the outcome.
    fn run(&mut self) -> CleanupOutcome {
        if !self.armed || self.keep {
            return CleanupOutcome::Skipped;
        }
        self.armed = false;
        match self.owned() {
            Some(true) if self.request_deletion() => CleanupOutcome::DeletionRequested,
            Some(true) => CleanupOutcome::DeletionNotAccepted,
            Some(false) => CleanupOutcome::NotOurs,
            None => CleanupOutcome::OwnershipUnknown,
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let group = self.group.clone();
        match self.run() {
            CleanupOutcome::Skipped => {}
            CleanupOutcome::DeletionRequested => eprintln!("cleanup: deletion of group {group} requested"),
            CleanupOutcome::DeletionNotAccepted => {
                eprintln!("cleanup: group {group} is ours but its deletion could not be requested; delete it by hand");
            }
            CleanupOutcome::NotOurs => eprintln!("cleanup: group {group} is absent or not ours; left untouched"),
            CleanupOutcome::OwnershipUnknown => {
                eprintln!("cleanup: ownership of group {group} could not be checked; inspect and delete it by hand");
            }
        }
    }
}

#[cfg(test)]
mod cleanup_tests {
    //! Deterministic coverage of the failure cleanup's decisions, with a scripted
    //! runner in place of `az`; these run in the ordinary matrix.
    use super::*;
    use std::{
        os::unix::process::ExitStatusExt as _,
        sync::{Arc, Mutex},
    };

    fn output(code: i32, stdout: &str, stderr: &str) -> std::process::Output {
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// A guard whose `az` answers `show` with `shown` and `delete` with `deleted`,
    /// recording every call.
    fn guard(
        shown: Option<std::process::Output>,
        deleted: Option<std::process::Output>,
    ) -> (Cleanup, Arc<Mutex<Vec<String>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&calls);
        let run_az: Runner = Box::new(move |args: &[&str], _: Instant| {
            seen.lock().unwrap().push(args[1].to_string());
            match args[1] {
                "show" => shown.clone(),
                "delete" => deleted.clone(),
                other => panic!("unexpected az call {other}"),
            }
        });
        let cleanup = Cleanup {
            subscription: "sub".into(),
            group: "horizon-ws-test".into(),
            workflow_id: "wf".into(),
            job_id: "job".into(),
            armed: true,
            keep: false,
            run_az,
        };
        (cleanup, calls)
    }

    fn owned_group() -> std::process::Output {
        output(0, r#"{"tags":{"horizon-workflow-id":"wf","horizon-job-id":"job"}}"#, "")
    }

    #[test]
    fn owned_group_is_deleted_and_the_acceptance_is_checked() {
        let (mut cleanup, calls) = guard(Some(owned_group()), Some(output(0, "", "")));
        assert_eq!(cleanup.run(), CleanupOutcome::DeletionRequested);
        assert_eq!(*calls.lock().unwrap(), ["show", "delete"]);
        assert_eq!(cleanup.run(), CleanupOutcome::Skipped, "acts once");
    }

    #[test]
    fn rejected_deletion_is_retried_then_reported_for_the_operator() {
        let (mut cleanup, calls) = guard(Some(owned_group()), Some(output(1, "", "throttled")));
        assert_eq!(cleanup.run(), CleanupOutcome::DeletionNotAccepted);
        assert_eq!(*calls.lock().unwrap(), ["show", "delete", "delete", "delete"]);
        let (mut cleanup, _) = guard(Some(owned_group()), None);
        assert_eq!(
            cleanup.run(),
            CleanupOutcome::DeletionNotAccepted,
            "a hung CLI is not an acceptance"
        );
    }

    #[test]
    fn foreign_and_absent_groups_are_left_untouched() {
        let foreign = Some(output(
            0,
            r#"{"tags":{"horizon-workflow-id":"other","horizon-job-id":"job"}}"#,
            "",
        ));
        let (mut cleanup, calls) = guard(foreign, Some(output(0, "", "")));
        assert_eq!(cleanup.run(), CleanupOutcome::NotOurs);
        assert_eq!(*calls.lock().unwrap(), ["show"], "no delete against a foreign group");
        let untagged = Some(output(0, r#"{"tags":null}"#, ""));
        let (mut cleanup, calls) = guard(untagged, Some(output(0, "", "")));
        assert_eq!(cleanup.run(), CleanupOutcome::NotOurs);
        assert_eq!(*calls.lock().unwrap(), ["show"]);
        let absent = Some(output(
            3,
            "",
            "(ResourceGroupNotFound) Resource group 'horizon-ws-test' could not be found.",
        ));
        let (mut cleanup, calls) = guard(absent, Some(output(0, "", "")));
        assert_eq!(cleanup.run(), CleanupOutcome::NotOurs);
        assert_eq!(*calls.lock().unwrap(), ["show"]);
    }

    #[test]
    fn unverifiable_ownership_never_deletes() {
        for shown in [
            None,
            Some(output(1, "", "AuthorizationFailed")),
            Some(output(0, "not json", "")),
        ] {
            let (mut cleanup, calls) = guard(shown, Some(output(0, "", "")));
            assert_eq!(cleanup.run(), CleanupOutcome::OwnershipUnknown);
            assert_eq!(*calls.lock().unwrap(), ["show"], "no delete without an ownership proof");
        }
    }

    #[test]
    fn disarmed_or_kept_guards_do_nothing() {
        let (mut cleanup, calls) = guard(Some(owned_group()), Some(output(0, "", "")));
        cleanup.armed = false;
        assert_eq!(cleanup.run(), CleanupOutcome::Skipped);
        let (mut cleanup, kept_calls) = guard(Some(owned_group()), Some(output(0, "", "")));
        cleanup.keep = true;
        assert_eq!(cleanup.run(), CleanupOutcome::Skipped);
        assert!(calls.lock().unwrap().is_empty() && kept_calls.lock().unwrap().is_empty());
    }
}

struct Live {
    run: Run,
    cleanup: std::cell::RefCell<Cleanup>,
    /// The exact deadline written on the VM for the reaper, fixed before creation.
    reaper_deadline: String,
    client: AzureClient,
    request: InteractiveWorkerRequest,
    group: String,
    subscription: String,
    private_key: std::path::PathBuf,
    directory: tempfile::TempDir,
}

fn settings() -> (AzureProfile, String) {
    let subscription = required("HORIZON_AZURE_LIVE_SUBSCRIPTION");
    let profile = AzureProfile {
        name: "live-acceptance".into(),
        subscription_id: subscription.clone(),
        location: optional("HORIZON_AZURE_LIVE_LOCATION", "northeurope"),
        vm_size: optional("HORIZON_AZURE_LIVE_VM_SIZE", "Standard_D2s_v3"),
        image_pull_identity_id: required("HORIZON_AZURE_LIVE_PULL_IDENTITY"),
        declared_hourly_cost_micros: optional("HORIZON_AZURE_LIVE_HOURLY_COST_MICROS", "120000")
            .parse()
            .expect("HORIZON_AZURE_LIVE_HOURLY_COST_MICROS must be a whole number"),
        registry_login_server: required("HORIZON_AZURE_LIVE_REGISTRY"),
        disk_sku: AzureDiskSku::default(),
    };
    (profile, subscription)
}

/// The reaper must exist and be armed before any VM does; the run refuses otherwise.
/// Like the spike harness, it resolves the job-schedule link of the reaper runbook for
/// this subscription and validates that linked schedule, so an unrelated schedule in
/// the same account cannot stand in for a working reaper.
fn preflight_reaper(subscription: &str, reaper_deadline: time::OffsetDateTime, run: &Run) {
    let group = optional("HORIZON_AZURE_LIVE_REAPER_GROUP", "horizon-worker-registry");
    let account = optional("HORIZON_AZURE_LIVE_REAPER_ACCOUNT", "horizon-spike-reaper");
    let base = format!(
        "https://management.azure.com/subscriptions/{subscription}/resourceGroups/{group}/providers/Microsoft.Automation/automationAccounts/{account}"
    );
    let get = |url: String| -> serde_json::Value {
        let output = az(
            &["rest", "--method", "get", "--url", &url, "-o", "json"],
            Instant::now() + CLI_STEP,
        )
        .expect("az rest finished within its bound");
        assert!(output.status.success(), "reaper lookup failed");
        serde_json::from_slice(&output.stdout).expect("reaper json")
    };
    let links = get(format!("{base}/jobSchedules?api-version=2023-11-01"));
    let schedule_name = links["value"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|link| link["properties"]["runbook"]["name"].as_str() == Some("horizon-spike-deadline-reaper"))
        .filter_map(|link| link["properties"]["jobScheduleId"].as_str().map(str::to_string))
        // The list omits parameters; each link is read by id, and Azure re-cases the key.
        .map(|id| get(format!("{base}/jobSchedules/{id}?api-version=2023-11-01")))
        .find(|link| {
            link["properties"]["parameters"]
                .as_object()
                .into_iter()
                .flatten()
                .any(|(key, value)| {
                    key.eq_ignore_ascii_case("subscriptionid")
                        && value
                            .as_str()
                            .is_some_and(|value| value.eq_ignore_ascii_case(subscription))
                })
        })
        .and_then(|link| link["properties"]["schedule"]["name"].as_str().map(str::to_string))
        .expect("the reaper runbook is linked to a schedule for this subscription; refusing to create");
    let schedule = get(format!("{base}/schedules/{schedule_name}?api-version=2023-11-01"));
    // The same facts the spike harness requires: enabled, a 15-minute cadence, and a
    // next run inside that cadence, so the tagged VM is reaped within one interval of
    // its deadline whatever happens to this controller.
    let now = time::OffsetDateTime::now_utc();
    let properties = &schedule["properties"];
    let instant = |field: &str| {
        properties[field]
            .as_str()
            .and_then(|value| time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok())
    };
    // Automation schedules can expire: it must still be running one interval after
    // the exact deadline that will be written on the VM. Azure reports "never" as a
    // far-future expiry.
    let must_run_until = reaper_deadline + time::Duration::minutes(15);
    let armed = properties["isEnabled"].as_bool() == Some(true)
        && properties["frequency"].as_str() == Some("Minute")
        && properties["interval"].as_u64() == Some(15)
        && instant("nextRun").is_some_and(|next| next > now && next <= now + time::Duration::minutes(16))
        && match &properties["expiryTime"] {
            serde_json::Value::Null => true,
            serde_json::Value::String(_) => instant("expiryTime").is_some_and(|expiry| expiry > must_run_until),
            // Any other shape is unreadable and counts as expired: fail closed.
            _ => false,
        };
    assert!(
        armed,
        "the reaper's linked schedule is not an enabled 15-minute schedule with a run due within the next interval and no expiry before the worker deadline; refusing to create"
    );
    run.event(
        "reaper_preflight",
        &format!(
            "runbook linked to schedule {schedule_name}: enabled, every 15 minutes, next run within the interval, no expiry before the worker deadline"
        ),
    );
}

fn setup() -> Live {
    let run = Run {
        started: Instant::now(),
    };
    let (profile, subscription) = settings();
    // Everything that could fail is read and range-checked before anything is billed.
    let reaper_hours: i64 = optional("HORIZON_AZURE_LIVE_REAPER_HOURS", "2")
        .parse()
        .expect("HORIZON_AZURE_LIVE_REAPER_HOURS must be a whole number of hours");
    assert!(
        (1..=24).contains(&reaper_hours),
        "HORIZON_AZURE_LIVE_REAPER_HOURS must be between 1 and 24"
    );
    // One deadline instant serves both the preflight and the tag on the VM.
    let reaper_deadline_at = time::OffsetDateTime::now_utc() + time::Duration::hours(reaper_hours);
    let reaper_deadline = reaper_deadline_at
        .format(&time::format_description::well_known::Rfc3339)
        .expect("deadline");
    preflight_reaper(&subscription, reaper_deadline_at, &run);
    let image = required("HORIZON_AZURE_LIVE_IMAGE");
    let directory = tempfile::tempdir().expect("temp dir");
    let (client_key, private_key) = generate_client_key(directory.path());
    let claimed = AtomicBool::new(false);
    let fence =
        move |_: CloudWorkflowId, _: CloudJobId, _: &WorkerTarget, _: &str| Ok(!claimed.swap(true, Ordering::SeqCst));
    let credential = AzureCliCredential::new(subscription.clone()).expect("credential");
    let client = AzureClient::new(profile, credential, fence).expect("client");
    let request = InteractiveWorkerRequest {
        workflow_id: CloudWorkflowId::new(),
        job_id: CloudJobId::new(),
        target: WorkerTarget {
            provider: CloudProvider::Azure,
            profile: "live-acceptance".into(),
            image,
            disk_gib: 32,
            lifetime: WorkerLifetime::Persistent,
            max_hourly_cost_micros: None,
        },
        ssh_public_key: client_key,
    };
    let group = resource_group_name(request.workflow_id, request.job_id);
    run.event("start", &format!("group {group}"));
    let cleanup = std::cell::RefCell::new(Cleanup::live(
        subscription.clone(),
        group.clone(),
        request.workflow_id.to_string(),
        request.job_id.to_string(),
    ));
    Live {
        run,
        cleanup,
        reaper_deadline,
        client,
        request,
        group,
        subscription,
        private_key,
        directory,
    }
}

impl Live {
    /// Create once, bound the VM's compute, and show a second ensure only reuses.
    fn create(&self) -> InteractiveWorker {
        // The exact group name is known before anything exists, and ensure mutates
        // (group, then deployment) before it answers: arm the cleanup first.
        self.cleanup.borrow_mut().armed = true;
        let created = self.client.ensure_worker(&self.request).expect("ensure");
        let InteractiveWorkerEnsure::Created(status) = &created else {
            panic!("first ensure must create: {created:?}");
        };
        self.run.event("created", &format!("{:?}", status.lifecycle));
        arm_reaper(&self.subscription, &self.group, &self.reaper_deadline, &self.run);
        let reused = self.client.ensure_worker(&self.request).expect("ensure again");
        assert!(matches!(reused, InteractiveWorkerEnsure::Reused(_)), "{reused:?}");
        self.run.event("reused", &format!("{:?}", reused.status().lifecycle));
        status.worker.clone()
    }

    /// Ready only with an attested host key, then that key on the wire.
    fn ready_and_prove_ssh(&self, persisted: &InteractiveWorker) -> InteractiveWorkerSshEndpoint {
        let ready: InteractiveWorkerStatus = wait_for(READY_BOUND, || {
            let status = self.client.reconcile_worker(&self.request)?.expect("worker present");
            Ok(match status.lifecycle {
                Lifecycle::Ready => Some(status),
                Lifecycle::Provisioning => None,
                other => panic!("unexpected lifecycle while waiting for ready: {other:?}"),
            })
        })
        .expect("ready");
        let endpoint = ready.ssh.as_ref().expect("endpoint");
        self.run.event(
            "ready",
            &format!("host key {} ... port {}", &endpoint.host_key[..24], endpoint.port),
        );
        assert!(ready.is_ready_for(&self.request, time::OffsetDateTime::now_utc()));
        let seen = ssh_proof(
            endpoint,
            &self.private_key,
            self.directory.path(),
            "echo live-marker > /workspace/.horizon-live-marker && cat /workspace/.horizon-live-marker && id -un",
        );
        assert_eq!(seen, "live-marker\nroot");
        self.run
            .event("ssh_proved", "pinned host key accepted, marker written to /workspace");
        let inspected = self
            .client
            .inspect_worker(persisted)
            .expect("inspect")
            .expect("present");
        assert_eq!(inspected.lifecycle, Lifecycle::Ready);
        self.run.event("inspected", "ready from the persisted handle");
        endpoint.clone()
    }

    /// Explicit compute start after the stop: the same worker comes back under the same
    /// address and host key, with the marker still on the retained disk; a second start
    /// finds it running and posts nothing.
    fn start_and_reattach(&self, persisted: &InteractiveWorker, before: &InteractiveWorkerSshEndpoint) {
        let started_at = Instant::now();
        let started = self.client.start_worker(persisted).expect("start");
        let InteractiveWorkerStart::Started(status) = &started else {
            panic!("a stopped worker is started: {started:?}");
        };
        self.run.event(
            "started",
            &format!(
                "running again in {:.0}s, observed {:?}",
                started_at.elapsed().as_secs_f64(),
                status.lifecycle
            ),
        );
        let ready: InteractiveWorkerStatus = wait_for(READY_BOUND, || {
            let status = self.client.inspect_worker(persisted)?.expect("worker present");
            Ok(match status.lifecycle {
                Lifecycle::Ready => Some(status),
                Lifecycle::Provisioning => None,
                other => panic!("unexpected lifecycle while waiting for ready after start: {other:?}"),
            })
        })
        .expect("ready after start");
        let endpoint = ready.ssh.as_ref().expect("endpoint");
        assert_eq!(
            (endpoint.host.as_str(), endpoint.port, endpoint.host_key.as_str()),
            (before.host.as_str(), before.port, before.host_key.as_str()),
            "same address and the same attested host key as before the stop"
        );
        self.run.event(
            "ready_after_start",
            &format!(
                "same endpoint and host key, {:.0}s after the start call",
                started_at.elapsed().as_secs_f64()
            ),
        );
        let seen = ssh_proof(
            endpoint,
            &self.private_key,
            self.directory.path(),
            "cat /workspace/.horizon-live-marker && id -un",
        );
        assert_eq!(
            seen, "live-marker\nroot",
            "the marker written before the stop is still there"
        );
        self.run
            .event("reattached", "pinned reattach with the retained marker on /workspace");
        let again = self.client.start_worker(persisted).expect("start again");
        assert!(matches!(again, InteractiveWorkerStart::AlreadyRunning(_)), "{again:?}");
        self.run.event(
            "start_idempotent",
            "a second start finds the worker running and posts nothing",
        );
    }

    /// Explicit stop: retained and verified; nothing afterwards creates.
    fn stop(&self, persisted: &InteractiveWorker) {
        let started = Instant::now();
        assert_eq!(
            self.client.stop_worker(persisted).expect("stop"),
            InteractiveWorkerStop::Stopped
        );
        self.run.event(
            "stopped",
            &format!("verified deallocated in {:.0}s", started.elapsed().as_secs_f64()),
        );
        let after = self
            .client
            .inspect_worker(persisted)
            .expect("inspect")
            .expect("present");
        assert_eq!(after.lifecycle, Lifecycle::Stopped);
        assert_eq!(
            self.client.stop_worker(persisted).expect("stop again"),
            InteractiveWorkerStop::Stopped
        );
        let ensured = self.client.ensure_worker(&self.request).expect("ensure stopped");
        assert!(matches!(ensured, InteractiveWorkerEnsure::Reused(_)));
        assert_eq!(ensured.status().lifecycle, Lifecycle::Stopped);
        self.run.event(
            "stopped_observed",
            "inspect, repeated stop and ensure all report the retained stop without creating",
        );
    }

    /// `Check saved Stop` against the live control plane: the read-only observation of
    /// the exact saved worker and pin answers as expected, and a pin whose address is
    /// not this worker's is an identity error rather than any observation.
    fn check_saved_stop(
        &self,
        persisted: &InteractiveWorker,
        endpoint: &InteractiveWorkerSshEndpoint,
        expected: InteractiveWorkerStopObservation,
        when: &str,
    ) {
        let observe = |ssh: &InteractiveWorkerSshEndpoint| {
            self.client.observe_worker_stop(InteractiveWorkerStopExpectation {
                worker: persisted,
                ssh,
                network_volume: None,
            })
        };
        let observed = observe(endpoint).expect("observe saved stop");
        assert_eq!(observed, expected, "check saved stop {when}");
        if expected == InteractiveWorkerStopObservation::RetainedStopped {
            // TEST-NET-3 is never an Azure public address: a saved pin that names
            // another host must be refused as this worker's identity, not observed.
            let moved = InteractiveWorkerSshEndpoint {
                host: "203.0.113.1".into(),
                ..endpoint.clone()
            };
            assert_eq!(observe(&moved), Err(AzureError::ResourceIdentityMismatch));
        }
        self.run.event(
            "saved_stop_checked",
            &format!("{when}: {observed:?} from the persisted handle and the saved pin"),
        );
    }

    /// Delete exactly the owned group and prove it is gone.
    fn delete(&self, persisted: &InteractiveWorker) {
        let started = Instant::now();
        let deleted = self.client.delete_worker(persisted).expect("delete");
        assert!(matches!(deleted, InteractiveWorkerCleanup::Deleted), "{deleted:?}");
        // Deletion is accepted and in flight: from here on it is the cleanup.
        self.cleanup.borrow_mut().armed = false;
        wait_for(GONE_BOUND, || {
            Ok(match self.client.inspect_worker(persisted)? {
                None => Some(()),
                Some(status) => {
                    assert_eq!(status.lifecycle, Lifecycle::Deleting, "{status:?}");
                    None
                }
            })
        })
        .expect("gone");
        let exists = az(
            &[
                "group",
                "exists",
                "--subscription",
                &self.subscription,
                "--name",
                &self.group,
                "-o",
                "tsv",
            ],
            Instant::now() + CLI_STEP,
        )
        .expect("az group exists finished within its bound");
        assert!(exists.status.success(), "az group exists failed");
        assert_eq!(String::from_utf8_lossy(&exists.stdout).trim(), "false");
        self.run.event(
            "deleted",
            &format!("group gone in {:.0}s", started.elapsed().as_secs_f64()),
        );
        assert_eq!(
            self.client.delete_worker(persisted).expect("delete again"),
            InteractiveWorkerCleanup::AlreadyAbsent
        );
    }
}

#[test]
#[ignore = "live Azure run; set HORIZON_AZURE_LIVE_* and pass --ignored"]
fn live_worker_create_ready_stop_delete() {
    let live = setup();
    let persisted = live.create();
    let endpoint = live.ready_and_prove_ssh(&persisted);
    live.check_saved_stop(
        &persisted,
        &endpoint,
        InteractiveWorkerStopObservation::Pending,
        "while running",
    );
    live.stop(&persisted);
    live.check_saved_stop(
        &persisted,
        &endpoint,
        InteractiveWorkerStopObservation::RetainedStopped,
        "after the stop",
    );
    live.start_and_reattach(&persisted, &endpoint);
    live.check_saved_stop(
        &persisted,
        &endpoint,
        InteractiveWorkerStopObservation::Pending,
        "after the start",
    );
    live.stop(&persisted);
    live.check_saved_stop(
        &persisted,
        &endpoint,
        InteractiveWorkerStopObservation::RetainedStopped,
        "after the second stop",
    );
    if keep() {
        live.cleanup.borrow_mut().armed = false;
        live.run
            .event("kept", &format!("group {} left in place on request", live.group));
        return;
    }
    live.delete(&persisted);
    live.check_saved_stop(
        &persisted,
        &endpoint,
        InteractiveWorkerStopObservation::Absent,
        "after the delete",
    );
    live.run.event(
        "done",
        "create, ready, pinned ssh, check saved stop, stop, start, pinned reattach with retained data, stop, delete all proven",
    );
}

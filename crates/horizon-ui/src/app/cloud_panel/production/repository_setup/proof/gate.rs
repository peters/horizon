//! The setup gate: Horizon's proof, and the desktop preview after a passed proof, run on
//! each repository of a list the way the New cloud dialog runs them. It runs only when
//! asked for, with `--ignored` and a list in `GATE_LIST`; `scripts/setup-gate.py` starts
//! it in parallel lanes and docs/testing/procedures/cloud-setup-gate.md tells how.
//!
//! - `GATE_LIST`: a JSON array of entries, each `{"repo", "path", "spec", "preview"?,
//!   "label"?, "run"?}`; `spec` and `preview` are what the agent writes in its status file.
//! - `GATE_OUT`: the folder for `started.jsonl`, `results.jsonl`, the proof logs and the
//!   preview screenshots.
//! - `GATE_PREFIX`: the start of the run names of entries without a `run`.
//! - `GATE_DOCKER_HOST`: the name of a Docker host saved in Cloud settings; when set,
//!   the proofs and previews run on its Docker, as a setup that builds on it does.
//! - `GATE_PROVE_AGAIN`: when set, a passed proof is followed by a second proof of the
//!   same commit in the same run, as when the agent proves again, and the result says
//!   what each checkout sent.
use super::super::preview;
use super::*;
use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// The longest one proof may take before the gate stops it and fails the entry: far
/// above the proof's own limits for each phase, so it ends only a proof that hangs and
/// never fails one that the dialog would pass.
const PROOF_LIMIT: Duration = Duration::from_hours(6);

/// The longest the desktop preview may take to show a window.
const PREVIEW_LIMIT: Duration = Duration::from_mins(10);

/// How often the gate takes what the proof or the preview reported.
const POLL: Duration = Duration::from_millis(500);

/// The Docker host `GATE_DOCKER_HOST` names in Cloud settings, if it names one.
fn docker_host() -> Option<horizon_core::cloud_runtime::docker_host::Host> {
    let name = std::env::var("GATE_DOCKER_HOST").ok().filter(|name| !name.is_empty())?;
    let path = horizon_core::HorizonHome::resolve()
        .root()
        .join("cloud")
        .join("settings.json");
    let settings = horizon_core::cloud_runtime::settings::Settings::load(&path).expect("Cloud settings load");
    let host = settings
        .docker_hosts
        .into_iter()
        .find(|host| host.name == name)
        .expect("GATE_DOCKER_HOST names a Docker host saved in Cloud settings");
    Some(host)
}

/// One repository of the list, as the gate proves it.
struct Entry {
    label: String,
    repository: PathBuf,
    spec: serde_json::Value,
    preview: Option<serde_json::Value>,
    /// The run name, from which Horizon names the entry's Docker resources.
    run: String,
}

impl Entry {
    fn read(value: &serde_json::Value, prefix: &str, index: usize) -> Self {
        let repo = value["repo"].as_str().expect("an entry names its repo");
        let path = value["path"].as_str().expect("an entry names its path");
        Self {
            label: value["label"].as_str().unwrap_or(repo).to_owned(),
            repository: std::fs::canonicalize(path).expect("the entry's path is a folder"),
            spec: value["spec"].clone(),
            preview: value.get("preview").filter(|preview| !preview.is_null()).cloned(),
            run: value["run"]
                .as_str()
                .map_or_else(|| format!("{prefix}{index:04}"), str::to_owned),
        }
    }

    /// The names of the Docker resources the entry's proof and preview can make, for the
    /// runner to remove when it stops a lane in the middle of this entry.
    fn names(&self) -> serde_json::Value {
        let (image, volume) = run_names(&self.repository, &self.run);
        serde_json::json!({
            "entry": self.label,
            "run": self.run,
            "image": image,
            "volume": volume,
            "checks": format!("horizon-contract-{}-", resources::check_operations(&volume)),
            "network": preview_network(&volume),
        })
    }
}

/// Appends `value` as one line to `file` in `out`, with one write, so the lanes that share
/// the file never mix their lines.
fn append(out: &Path, file: &str, value: &serde_json::Value) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out.join(file))
        .unwrap();
    file.write_all(format!("{value}\n").as_bytes()).unwrap();
}

#[test]
#[ignore = "proves real repositories; scripts/setup-gate.py starts it"]
fn proves_each_listed_repository() {
    let Ok(list) = std::env::var("GATE_LIST") else {
        eprintln!("GATE_LIST names no list, so the gate proves nothing");
        return;
    };
    let entries: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(list).unwrap()).unwrap();
    let out = PathBuf::from(std::env::var("GATE_OUT").expect("GATE_OUT names the output folder"));
    let prefix = std::env::var("GATE_PREFIX").unwrap_or_else(|_| "gate".to_owned());
    std::fs::create_dir_all(out.join("logs")).unwrap();
    for (index, value) in entries.iter().enumerate() {
        let entry = Entry::read(value, &prefix, index);
        append(&out, "started.jsonl", &entry.names());
        let started = Instant::now();
        let mut result = prove(&entry, &out);
        result["entry"] = entry.label.clone().into();
        result["run"] = entry.run.clone().into();
        result["seconds"] = started.elapsed().as_secs().into();
        append(&out, "results.jsonl", &result);
    }
    // No SSH forward to a Docker host outlives the gate.
    super::remote::close(true);
}

/// Proves the entry, starts its preview after a passed proof and removes all the entry's
/// Docker resources; the entry's result.
fn prove(entry: &Entry, out: &Path) -> serde_json::Value {
    let Some(spec) = Spec::parse(&entry.spec) else {
        return serde_json::json!({"passed": false, "error": "the spec does not parse"});
    };
    let log = out.join("logs").join(format!("{}.log", entry.label));
    let identity = super::super::untrusted::identity(&entry.repository);
    // The person allows the clones of the private Rust git repositories the locks pin,
    // and the restore of private .NET packages on this computer.
    let access = crates::Access {
        repositories: crates::locked_repositories(&entry.repository, "HEAD"),
        forget_volume: false,
        allowed: Arc::new(AtomicBool::new(true)),
    };
    let root_dirty = Arc::new(AtomicBool::new(false));
    // Each proof of the run has its own request number, and so its own container names.
    let start = |request: u64, log: &Path, access: crates::Access| {
        Proof::start(
            (&entry.repository, &entry.run, identity),
            docker_host(),
            (request, spec.clone()),
            log.to_owned(),
            (true, root_dirty.clone(), access),
            (None, egui::Context::default()),
        )
    };
    let mut proof = start(1, &log, access.clone());
    let ended = wait(&mut proof);
    let phases: Vec<serde_json::Value> = proof
        .phases
        .iter()
        .map(|(phase, state)| {
            serde_json::json!({
                "phase": phase.title(),
                "seconds": state.ended.zip(state.started).map(|(end, start)| (end - start).as_secs()),
                "failure": state.failure,
            })
        })
        .collect();
    let proved = proof
        .phases
        .iter()
        .all(|(_, state)| state.ended.is_some() && state.failure.is_none());
    let pinned = proof.daemon();
    let preview = match (&entry.preview, proved) {
        (Some(value), true) => {
            let screenshot = out.join("logs").join(format!("{}.preview.png", entry.label));
            show_preview(&proof, &pinned, &root_dirty, value, screenshot)
        }
        _ => serde_json::Value::Null,
    };
    // A preview that could not run here, such as one that needs /dev/kvm, does not fail
    // the entry, as the dialog accepts such a setup.
    let mut passed = proved && preview["outcome"] != "failed";
    let mut worker = proof.stop();
    let mut pinned = pinned;
    let mut again = None;
    if passed && std::env::var_os("GATE_PROVE_AGAIN").is_some() {
        // The run's volume and its git cache stay; the earlier proof has ended first.
        if let Some(worker) = worker.take() {
            let _ = worker.join();
        }
        let again_log = out.join("logs").join(format!("{}.again.log", entry.label));
        let mut second = start(2, &again_log, access);
        let (proved, value, error) = again_result(&mut second, (&log, &again_log));
        passed = proved;
        again = Some((value, error));
        pinned = second.daemon();
        worker = second.stop();
    }
    remove_run_resources(&entry.repository, &entry.run, worker, (Some(pinned), None), true);
    let mut result = serde_json::json!({
        "passed": passed,
        "phases": phases,
        "preview": preview,
        "log": log,
    });
    if let Some((value, error)) = again {
        result["again"] = value;
        if let Some(error) = error {
            result["error"] = error.into();
        }
    }
    if !ended {
        result["error"] = format!("the proof did not end within {} minutes", PROOF_LIMIT.as_secs() / 60).into();
    }
    result
}

/// Waits for the second proof of an entry; whether it passed, the `again` value of the
/// entry's result with what each checkout sent, and the entry's error when it failed: the
/// summary names the second proof's failure, never the first proof's phases.
fn again_result(second: &mut Proof, (log, again_log): (&Path, &Path)) -> (bool, serde_json::Value, Option<String>) {
    let ended = wait(second);
    let proved = ended
        && second
            .phases
            .iter()
            .all(|(_, state)| state.ended.is_some() && state.failure.is_none());
    let error = (!proved).then(|| {
        let failure = second
            .phases
            .iter()
            .find_map(|(phase, state)| {
                state
                    .failure
                    .as_ref()
                    .map(|failure| format!("{}: {failure}", phase.title()))
            })
            .unwrap_or_else(|| "it did not end".to_owned());
        // The log first: the summary shows the first line only.
        format!(
            "the second proof of the same commit failed (see {}): {failure}",
            again_log.display()
        )
    });
    let value = serde_json::json!({
        "passed": proved,
        "sent": [sent(log), sent(again_log)],
        "log": again_log,
    });
    (proved, value, error)
}

/// Polls `proof` until each phase ended or one failed, within [`PROOF_LIMIT`]; whether it
/// ended.
fn wait(proof: &mut Proof) -> bool {
    let begun = Instant::now();
    loop {
        proof.poll();
        let ended = proof.phases.iter().all(|(_, state)| state.ended.is_some())
            || proof.phases.iter().any(|(_, state)| state.failure.is_some());
        if ended || begun.elapsed() > PROOF_LIMIT {
            return ended;
        }
        std::thread::sleep(POLL);
    }
}

/// What the checkouts of the proof that logged to `log` sent, as their log lines say.
fn sent(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains("for the checkout") || line.contains("git cache does not hold"))
        .map(|line| line.trim().to_owned())
        .collect()
}

/// Starts the desktop preview of the proved image as the dialog does, waits for a window
/// and stops the preview again; what it reached.
fn show_preview(
    proof: &Proof,
    pinned: &Pinned,
    root_dirty: &Arc<AtomicBool>,
    value: &serde_json::Value,
    screenshot: PathBuf,
) -> serde_json::Value {
    let Some(spec) = preview::Spec::parse(value) else {
        return serde_json::json!({"outcome": "failed", "detail": "the preview spec does not parse"});
    };
    let mut preview = preview::Preview::start(
        (
            &proof.image,
            &proof.volume,
            pinned.clone(),
            proof.gpu,
            root_dirty.clone(),
        ),
        (1, spec),
        screenshot,
        (None, egui::Context::default()),
    );
    let begun = Instant::now();
    // The last look comes after the last pause, so a window that drew in it counts.
    loop {
        preview.poll();
        if !matches!(preview.outcome, preview::Outcome::Starting) || begun.elapsed() >= PREVIEW_LIMIT {
            break;
        }
        std::thread::sleep(POLL);
    }
    let outcome = match &preview.outcome {
        preview::Outcome::Live {
            window,
            after,
            screenshot,
            ..
        } => serde_json::json!({
            "outcome": "live",
            "window": window,
            "seconds": after.as_secs(),
            "screenshot": screenshot,
        }),
        preview::Outcome::Unavailable(reason) => serde_json::json!({"outcome": "unavailable", "detail": reason}),
        preview::Outcome::Failed(reason) => serde_json::json!({"outcome": "failed", "detail": reason}),
        preview::Outcome::Starting => {
            let detail = format!("no window within {} minutes", PREVIEW_LIMIT.as_secs() / 60);
            serde_json::json!({"outcome": "failed", "detail": detail})
        }
    };
    // The preview's container goes before the run's resources do.
    let _ = preview.stop().join();
    outcome
}

#[test]
fn the_runner_names_the_worker_checks_as_the_proof_does() {
    // scripts/test_setup_gate.py checks the same pair against the runner's copy of the hash,
    // with which it refuses a started record that names another run's check containers.
    assert_eq!(
        resources::check_operations("horizon-setup-example-cli-c357f1-gate-312b8e-1-0000"),
        "horizon-setup-check-8c80ce35"
    );
}

//! What the agent asks Horizon to prove: CI jobs for localci, or plain commands. The
//! agent writes it, and localci or bash reads it, so only plain names and bounded
//! one-line commands pass.
use std::path::Path;

/// What the agent asks Horizon to prove, from the status file's `proof`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production::repository_setup) struct Spec {
    pub(in crate::app::cloud_panel::production::repository_setup) workflow: String,
    pub(in crate::app::cloud_panel::production::repository_setup) jobs: Vec<String>,
    pub(in crate::app::cloud_panel::production::repository_setup) event: String,
    /// Steps that throw caches away on purpose; a worker leaves them out.
    pub(in crate::app::cloud_panel::production::repository_setup) skip: Vec<String>,
    /// For a repository without GitHub Actions: the shell commands that build and test
    /// it, run in order in place of localci. Empty when the proof is a workflow.
    pub(in crate::app::cloud_panel::production::repository_setup) commands: Vec<String>,
}

/// A workflow file name in `.github/workflows`: no folder, no option, YAML.
fn workflow_name(name: &str) -> bool {
    let allowed = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-');
    let yaml = Path::new(name)
        .extension()
        .is_some_and(|extension| extension == "yml" || extension == "yaml");
    !name.starts_with(['.', '-']) && yaml && name.bytes().all(allowed)
}

/// The most steps a proof may leave out.
const SKIP_LIMIT: usize = 20;

/// A step to leave out, which localci matches as a part of a step's name: never blank or
/// so short that it matches nearly every step, so leaving steps out cannot empty a job.
fn skip_name(step: &str) -> bool {
    let length = step.trim().chars().count();
    (3..=200).contains(&length)
}

/// A localci job: its id, `#N` for a matrix entry, never an option or a word localci
/// reads as a command.
fn job_name(name: &str) -> bool {
    let allowed = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'#' | b':');
    !name.is_empty()
        && !name.starts_with(['-', '.', '#'])
        && !matches!(name, "help" | "version" | "list" | "plan" | "run" | "check")
        && name.bytes().all(allowed)
}

/// The most commands a proof without a workflow may run.
const MAX_COMMANDS: usize = 32;

/// The longest command a proof may run.
const MAX_COMMAND_LENGTH: usize = 4096;

impl Spec {
    pub(in crate::app::cloud_panel::production::repository_setup) fn parse(value: &serde_json::Value) -> Option<Self> {
        // Every list must hold only strings: a malformed entry is never dropped quietly,
        // so the proof runs all that was asked or nothing.
        let strings = |key: &str| -> Option<Vec<String>> {
            match value.get(key) {
                None | Some(serde_json::Value::Null) => Some(Vec::new()),
                Some(serde_json::Value::Array(items)) => {
                    items.iter().map(|item| item.as_str().map(str::to_owned)).collect()
                }
                Some(_) => None,
            }
        };
        let text = |key: &str, default: &str| -> Option<String> {
            match value.get(key) {
                None | Some(serde_json::Value::Null) => Some(default.to_owned()),
                Some(item) => item.as_str().map(|text| text.trim().to_owned()),
            }
        };
        let (commands, jobs, skip) = (strings("commands")?, strings("jobs")?, strings("skip")?);
        let (workflow, event) = (text("workflow", "")?, text("event", "push")?);
        // The agent writes these and localci reads them as its arguments: a workflow is a
        // file name in .github/workflows, a job and the event are names, never options.
        // Without a workflow, plain commands prove it: a few, each one bounded line. The
        // two are alternatives; a proof that names both is refused.
        let event_ok = !event.is_empty() && event.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_');
        let workflow_ok = workflow_name(&workflow)
            && !jobs.is_empty()
            && jobs.iter().all(|job| job_name(job))
            && skip.len() <= SKIP_LIMIT
            && skip.iter().all(|step| skip_name(step));
        let commands_ok = !commands.is_empty()
            && commands.len() <= MAX_COMMANDS
            && commands.iter().all(|command| {
                !command.trim().is_empty()
                    && command.chars().count() <= MAX_COMMAND_LENGTH
                    && !command.contains(['\0', '\n', '\r'])
            });
        let workflow_named = !workflow.is_empty() || !jobs.is_empty() || !skip.is_empty();
        let valid = event_ok
            && if commands.is_empty() {
                workflow_ok
            } else {
                commands_ok && !workflow_named
            };
        valid.then_some(Self {
            workflow,
            jobs,
            event,
            skip,
            commands,
        })
    }

    /// The localci arguments for this proof's JSON plan, which shows what would run.
    pub(super) fn plan_args(&self) -> Vec<String> {
        let mut args = vec!["plan".to_owned(), "-w".to_owned(), self.workflow.clone()];
        args.extend(self.jobs.iter().cloned());
        args.extend(["--event".to_owned(), self.event.clone()]);
        for step in &self.skip {
            args.extend(["--skip".to_owned(), step.clone()]);
        }
        args.push("--json".to_owned());
        args
    }

    /// The localci arguments for this proof; `fast` for the warm rerun, which compares
    /// against `base`, the proved commit, since the clone's own default branch is
    /// whatever this computer's checkout last fetched.
    pub(super) fn localci_args(&self, fast: bool, base: &str) -> Vec<String> {
        let mut args = vec!["run".to_owned(), "-w".to_owned(), self.workflow.clone()];
        args.extend(self.jobs.iter().cloned());
        args.extend(["--event".to_owned(), self.event.clone()]);
        for step in &self.skip {
            args.extend(["--skip".to_owned(), step.clone()]);
        }
        if fast {
            args.extend(["--fast".to_owned(), "--base".to_owned(), base.to_owned()]);
        } else {
            // The full run's report, which the proof checks after it.
            args.extend(["--cache".to_owned(), "--json".to_owned()]);
        }
        args
    }
}

/// The shell script of a proof with commands: each command in a Bash of its own, with
/// `-e` and `pipefail`, so a failing one stops the proof, also inside a pipeline such as
/// `make test | tee log`, and no command can end, hide or swallow the ones after it
/// (`exit 0`, a comment, a trailing backslash). Each starts in the checkout.
pub(super) fn commands_script(commands: &[String]) -> String {
    let quote = |command: &str| format!("'{}'", command.replace('\'', "'\\''"));
    let lines = commands
        .iter()
        .map(|command| format!("bash -eo pipefail -c {}", quote(command)))
        .collect::<Vec<_>>()
        .join("\n");
    format!("set -e\n{lines}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proof_names_a_workflow_and_jobs() {
        let spec = Spec::parse(&serde_json::json!({
            "workflow": "dotnet-core.yml",
            "jobs": ["setup", "test"],
            "skip": ["Use isolated NuGet caches"]
        }))
        .unwrap();
        assert_eq!(spec.event, "push", "push by default");
        assert_eq!(
            spec.localci_args(true, "abc123"),
            [
                "run",
                "-w",
                "dotnet-core.yml",
                "setup",
                "test",
                "--event",
                "push",
                "--skip",
                "Use isolated NuGet caches",
                "--fast",
                "--base",
                "abc123"
            ]
        );
        assert!(
            spec.localci_args(false, "abc123")
                .ends_with(&["--cache".to_owned(), "--json".to_owned()]),
            "the full run reports as JSON for the proof's check"
        );
        assert!(Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": []})).is_none());
        let commands =
            Spec::parse(&serde_json::json!({"commands": ["pip install -r requirements.txt", "pytest -q"]})).unwrap();
        assert_eq!(
            commands.commands.len(),
            2,
            "a repository without CI proves with commands"
        );
        let many: Vec<String> = (0..=MAX_COMMANDS).map(|n| format!("echo {n}")).collect();
        assert!(
            Spec::parse(&serde_json::json!({ "commands": many })).is_none(),
            "a few commands"
        );
        assert!(Spec::parse(&serde_json::json!({"commands": ["make\nrm -rf /"]})).is_none());
        assert!(
            Spec::parse(&serde_json::json!({"workflow": "../x.yml", "commands": ["make"]})).is_none(),
            "an invalid workflow is refused, not dropped for the commands"
        );
        assert!(Spec::parse(&serde_json::json!({"workflow": "../x.yml", "jobs": ["a"]})).is_none());
        for job in ["--help", "help", "version", "-k"] {
            assert!(
                Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": [job]})).is_none(),
                "{job} would end localci without a job"
            );
        }
        for workflow in ["/etc/ci.yml", "sub/ci.yml", "-w.yml", "ci.txt"] {
            assert!(Spec::parse(&serde_json::json!({"workflow": workflow, "jobs": ["a"]})).is_none());
        }
        assert!(Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["rust-test#2"]})).is_some());
        for skip in ["", "   ", "t"] {
            assert!(
                Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["test"], "skip": [skip]})).is_none(),
                "{skip:?} would leave out every step"
            );
        }
        assert!(
            Spec::parse(&serde_json::json!({"commands": ["build", 1, "test"]})).is_none(),
            "a malformed entry refuses the list, it is not dropped"
        );
        assert!(Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["a", null]})).is_none());
        assert!(
            Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["a"], "commands": ["make"]})).is_none(),
            "a workflow and commands are alternatives"
        );
    }

    // The script runs with bash, which Windows does not have by default.
    #[cfg(unix)]
    #[test]
    fn the_commands_run_each_on_its_own_and_fail_inside_a_pipeline() {
        use std::process::Command;
        let script = commands_script(&[
            "echo one # a comment".to_owned(),
            "false".to_owned(),
            "echo three".to_owned(),
        ]);
        let output = Command::new("bash").arg("-c").arg(&script).output().unwrap();
        assert!(
            !output.status.success(),
            "the command after a comment still runs, and its failure stops the proof"
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "one\n");
        let ended = commands_script(&["exit 0".to_owned(), "echo second; false".to_owned()]);
        let output = Command::new("bash").arg("-c").arg(&ended).output().unwrap();
        assert!(!output.status.success(), "exit 0 in one command does not skip the next");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "second\n");
        let merged = commands_script(&["echo 'it'\\".to_owned(), "false".to_owned()]);
        let output = Command::new("bash").arg("-c").arg(&merged).output().unwrap();
        assert!(
            !output.status.success(),
            "a trailing backslash does not swallow the next command"
        );
        let piped = commands_script(&["false | true".to_owned(), "echo after".to_owned()]);
        let output = Command::new("bash").arg("-c").arg(&piped).output().unwrap();
        assert!(!output.status.success(), "a failure inside a pipeline fails the proof");
        assert!(output.stdout.is_empty());
    }
}

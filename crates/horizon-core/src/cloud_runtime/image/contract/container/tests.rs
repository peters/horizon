use super::*;
use crate::cloud_runtime::Event;
use horizon_cloud::CloudError;

const OPERATION: &str = "00000000-0000-4000-8000-000000000001";
const OTHER: &str = "00000000-0000-4000-8000-000000000002";

/// A Docker daemon as far as contract checks see it. `containers/<name>` is a listed
/// container holding its operation label; `reserved/<name>` is a name an unfinished create
/// holds, which no listing shows and `rm` cannot remove. A create of a held name conflicts.
/// With `HANG` a create holds its name, says `creating` and never finishes; with `STUCK`
/// `rm` removes nothing. With `FINISH` an `rm` lets the daemon finish the creates it holds.
const FAKE: &str = r#"#!/bin/sh
echo "$*" >> "$DIR/log"
case "$1 $2" in
  "create --name")
    name=$3; label=$5
    if [ -e "$DIR/containers/$name" ] || [ -e "$DIR/reserved/$name" ]; then
      echo "Error response from daemon: Conflict. The container name \"/$name\" is already in use" >&2
      exit 1
    fi
    if [ -n "$HANG" ]; then touch "$DIR/reserved/$name"; echo creating; sleep 10; exit 1; fi
    printf '%s' "${label#horizon.contract.operation=}" > "$DIR/containers/$name" ;;
  "start --attach") [ -e "$DIR/containers/$3" ] && echo contract-report ;;
  "container ls")
    case "$6" in
      label=horizon.contract.operation=*)
        for container in "$DIR"/containers/*; do
          [ -e "$container" ] && [ "$(cat "$container")" = "${6#label=horizon.contract.operation=}" ] && basename "$container"
        done ;;
      name=*) name=${6#name=^/}; name=${name%\$}; [ -e "$DIR/containers/$name" ] && echo "$name" ;;
    esac
    exit 0 ;;
  "container rm")
    shift 4
    # Like Docker, rm removes every container it can and fails when one is missing.
    status=0
    for name in "$@"; do [ -e "$DIR/containers/$name" ] || status=1; done
    if [ -z "$STUCK" ]; then for name in "$@"; do rm -f "$DIR/containers/$name"; done; fi
    if [ -n "$FINISH" ]; then
      for held in "$DIR"/reserved/*; do
        [ -e "$held" ] && rm "$held" && printf '%s' "$OPERATION" > "$DIR/containers/$(basename "$held")"
      done
    fi
    exit $status ;;
esac
"#;

struct Daemon {
    root: tempfile::TempDir,
    hang: bool,
    stuck: bool,
    finish: bool,
}

impl Daemon {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("docker"), FAKE).unwrap();
        for directory in ["containers", "reserved"] {
            std::fs::create_dir(root.path().join(directory)).unwrap();
        }
        Self {
            root,
            hang: false,
            stuck: false,
            finish: false,
        }
    }

    fn docker(&self) -> impl Fn() -> Command + '_ {
        let flag = |set: bool| if set { "1" } else { "" };
        move || {
            // Run through the shell rather than executing the script just written, which a
            // concurrent test's fork can still hold open for writing (ETXTBSY).
            let mut command = Command::new("/bin/sh");
            command
                .arg(self.root.path().join("docker"))
                .env("DIR", self.root.path())
                .env("HANG", flag(self.hang))
                .env("STUCK", flag(self.stuck))
                .env("FINISH", flag(self.finish))
                .env("OPERATION", OPERATION);
            command
        }
    }

    fn check(&self, cancel: &Cancellation, emit: &dyn Fn(Event)) -> Result<String> {
        let runner = Runner {
            cancel,
            emit,
            secrets: Vec::new(),
        };
        Checks {
            docker: self.docker(),
            runner: &runner,
            operation_id: OPERATION,
        }
        .run("registry.example/worker@sha256:t", &capabilities(), false, |output| {
            Ok(output.trim().to_owned())
        })
    }

    fn container(&self, name: &str, operation: &str) {
        std::fs::write(self.directory("containers").join(name), operation).unwrap();
    }

    fn names(&self, directory: &str) -> Vec<String> {
        let mut names = std::fs::read_dir(self.directory(directory))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn directory(&self, name: &str) -> std::path::PathBuf {
        self.root.path().join(name)
    }

    /// The names the checks created, in order.
    fn created(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.path().join("log"))
            .unwrap()
            .lines()
            .filter_map(|line| line.strip_prefix("create --name "))
            .map(|rest| rest.split(' ').next().unwrap().to_owned())
            .collect()
    }

    /// The daemon finishes the creates it holds names for.
    fn finish_creates(&self) {
        for name in self.names("reserved") {
            std::fs::rename(
                self.directory("reserved").join(&name),
                self.directory("containers").join(&name),
            )
            .unwrap();
            std::fs::write(self.directory("containers").join(name), OPERATION).unwrap();
        }
    }
}

fn capabilities() -> Capabilities {
    horizon_cloud::CloudConfig::parse(horizon_cloud::EXAMPLE)
        .unwrap()
        .profiles
        .remove("image-only")
        .unwrap()
        .capabilities
}

fn check(daemon: &Daemon) -> Result<String> {
    daemon.check(&Cancellation::default(), &|_| {})
}

fn assert_named_for_the_operation(name: &str) {
    let suffix = name
        .strip_prefix(&format!("horizon-contract-{OPERATION}-"))
        .unwrap_or_else(|| panic!("unexpected name {name}"));
    assert!(horizon_cloud::valid_id(suffix), "{name}");
}

#[test]
fn every_check_runs_in_a_container_of_its_own_that_it_removes() {
    let daemon = Daemon::new();
    assert_eq!(check(&daemon).unwrap(), "contract-report");
    assert_eq!(check(&daemon).unwrap(), "contract-report");
    let created = daemon.created();
    assert_eq!(created.len(), 2);
    assert_ne!(created[0], created[1]);
    for name in &created {
        assert_named_for_the_operation(name);
    }
    assert_eq!(daemon.names("containers"), Vec::<String>::new());
}

#[test]
fn a_retry_after_a_cancelled_create_does_not_conflict_with_the_name_the_daemon_still_holds() {
    let mut daemon = Daemon::new();
    daemon.hang = true;
    let cancel = Cancellation::default();
    let emit = |event| {
        if matches!(&event, Event::Output(line) if line == "creating") {
            cancel.cancel();
        }
    };
    assert!(matches!(
        daemon.check(&cancel, &emit),
        Err(Error::Provider(CloudError::Cancelled))
    ));
    // The cancelled create still holds its name: its own cleanup could not see it.
    let held = daemon.names("reserved");
    assert_eq!(held, daemon.created());
    daemon.hang = false;
    assert_eq!(check(&daemon).unwrap(), "contract-report");
    assert_eq!(daemon.names("reserved"), held);
    // Once the daemon has created it, the next check removes it.
    daemon.finish_creates();
    assert_eq!(daemon.names("containers"), held);
    assert_eq!(check(&daemon).unwrap(), "contract-report");
    assert_eq!(daemon.names("containers"), Vec::<String>::new());
}

#[test]
fn the_shared_name_an_earlier_version_left_held_no_longer_blocks_a_check() {
    let daemon = Daemon::new();
    std::fs::write(
        daemon
            .directory("reserved")
            .join(format!("horizon-contract-{OPERATION}")),
        "",
    )
    .unwrap();
    assert_eq!(check(&daemon).unwrap(), "contract-report");
}

#[test]
fn a_check_removes_what_earlier_checks_of_its_operation_left_and_nothing_else() {
    let daemon = Daemon::new();
    let earlier = format!("horizon-contract-{OPERATION}-earlier");
    let shared = format!("horizon-contract-{OPERATION}");
    let other = format!("horizon-contract-{OTHER}-live");
    daemon.container(&earlier, OPERATION);
    // An earlier version created its containers without the label.
    daemon.container(&shared, "");
    daemon.container(&other, OTHER);
    assert_eq!(check(&daemon).unwrap(), "contract-report");
    assert_eq!(daemon.names("containers"), [other]);
}

#[test]
fn a_container_that_stays_listed_fails_the_cleanup() {
    let mut daemon = Daemon::new();
    daemon.container(&format!("horizon-contract-{OPERATION}-earlier"), OPERATION);
    daemon.stuck = true;
    let error = check(&daemon).unwrap_err();
    assert_eq!(error.to_string(), "Worker image contract container cleanup failed");
    assert_eq!(daemon.created(), Vec::<String>::new());
}

#[test]
fn a_held_create_the_daemon_finishes_during_cleanup_is_removed_by_the_next_check() {
    let mut daemon = Daemon::new();
    std::fs::write(
        daemon
            .directory("reserved")
            .join(format!("horizon-contract-{OPERATION}-killed")),
        "",
    )
    .unwrap();
    // The daemon finishes it while the check removes its own container.
    daemon.finish = true;
    assert_eq!(check(&daemon).unwrap(), "contract-report");
    let finished = format!("horizon-contract-{OPERATION}-killed");
    assert_eq!(daemon.names("containers"), [finished]);
    daemon.finish = false;
    assert_eq!(check(&daemon).unwrap(), "contract-report");
    assert_eq!(daemon.names("containers"), Vec::<String>::new());
}

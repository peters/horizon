use super::*;
use crate::cloud_runtime::new_id;
use std::{cell::RefCell, os::unix::fs::PermissionsExt};

const LOCAL: [&str; 2] = ["horizon-layer:t-0", "horizon-layer:t-1"];
const REMOVE: &str = "image rm horizon-layer:t-0 horizon-layer:t-1";
const VERIFY: &str = "image ls --format {{.Repository}}:{{.Tag}} --filter reference=horizon-layer:t-0 --filter reference=horizon-layer:t-1";
const FAKE: &str = r#"#!/bin/sh
echo "$*" >> "$DIR/log"
case "$1 $2" in
  "buildx inspect") printf 'Name: default\nDriver: %s\n' "$DRIVER" ;;
  "buildx build")
    case "$* " in *"--tag $FAIL_TAG "*) exit 1 ;; esac
    if [ -n "$BLOCK" ]; then touch "$DIR/building"; sleep 10; fi
    previous=; tag=; base=; label=
    for argument in "$@"; do
      case "$previous" in
        --tag) tag=$argument ;;
        --label) label=$argument ;;
        --build-arg) case "$argument" in HORIZON_BASE=*) base=${argument#HORIZON_BASE=} ;; esac ;;
      esac
      previous=$argument
    done
    labels="$DIR/labels-$(echo "$tag" | tr '/:' '__')"
    : > "$labels"
    if [ -n "$base" ] && [ -z "$UNRELATED" ]; then cat "$DIR/labels-$(echo "$base" | tr '/:' '__')" >> "$labels"; fi
    if [ -n "$label" ]; then echo "$label" >> "$labels"; fi ;;
  "image rm") exit 1 ;;
  "image ls")
    if [ -n "$STALE" ] || { [ -n "$BLOCK" ] && [ -e "$DIR/building" ]; }; then echo horizon-layer:t-0; fi ;;
  "image inspect")
    case "$4" in
      "{{.Id}}") echo sha256:final ;;
      *) awk -F= 'BEGIN { printf "{" } { printf "%s\"%s\":\"%s\"", (NR > 1 ? "," : ""), $1, $2 } END { print "}" }' \
           "$DIR/labels-$(echo "$5" | tr '/:' '__')" ;;
    esac ;;
esac
"#;

fn recipe() -> Recipe {
    Recipe {
        dockerfile: "/snapshot/.horizon/Dockerfile".into(),
        context: "/snapshot/.horizon".into(),
        platform: "linux/amd64".into(),
    }
}

fn layers() -> [(&'static str, Recipe); 2] {
    [("one", recipe()), ("two", recipe())]
}

fn runner<'a>(cancel: &'a Cancellation, emit: &'a dyn Fn(Event)) -> Runner<'a> {
    Runner {
        cancel,
        emit,
        secrets: Vec::new(),
    }
}

/// How the fake `docker` behaves: `buildx inspect` reports `driver`, a build tagging
/// `fail_tag` fails, with `block` a build blocks after marking `building` and from then
/// on the first layer stays listed, with `stale` it is always listed, and with
/// `unrelated` no image inherits its base's labels. Otherwise an image's labels are its
/// base's plus its own `--label`.
struct Fake {
    root: tempfile::TempDir,
    driver: &'static str,
    fail_tag: &'static str,
    block: bool,
    stale: bool,
    unrelated: bool,
}

impl Fake {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("docker");
        std::fs::write(&script, FAKE).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            driver: "docker",
            fail_tag: "none",
            block: false,
            stale: false,
            unrelated: false,
        }
    }

    fn docker(&self) -> impl Fn() -> Command + '_ {
        let flag = |set: bool| if set { "1" } else { "" };
        move || {
            let mut command = Command::new(self.root.path().join("docker"));
            command
                .env("DIR", self.root.path())
                .env("DRIVER", self.driver)
                .env("FAIL_TAG", self.fail_tag)
                .env("BLOCK", flag(self.block))
                .env("STALE", flag(self.stale))
                .env("UNRELATED", flag(self.unrelated));
            command
        }
    }

    fn build(&self, emit: &dyn Fn(Event), cancel: &Cancellation) -> Result<String> {
        let runner = runner(cancel, emit);
        Builder {
            docker: self.docker(),
            runner: &runner,
        }
        .build_layers("registry.example/worker:t", "t", &recipe(), &layers(), &[])
    }

    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.path().join("log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

#[test]
fn a_build_without_layering_keeps_its_exact_command_and_layering_follows_the_arguments() {
    let cancel = Cancellation::default();
    let runner = runner(&cancel, &|_| {});
    let builder = Builder {
        docker: || {
            let mut command = Command::new("docker");
            command.args(["--config", "/synthetic/config", "--host", "unix:///synthetic.sock"]);
            command
        },
        runner: &runner,
    };
    let arguments = ["--build-arg".to_owned(), "HORIZON_DESKTOP=true".to_owned()];
    let argv = |layering: &[String]| {
        builder
            .build_command("registry.example/worker:horizon-cloud", &recipe(), &arguments, layering)
            .get_args()
            .map(|argument| argument.to_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let mut expected = vec![
        "--config",
        "/synthetic/config",
        "--host",
        "unix:///synthetic.sock",
        "buildx",
        "build",
        "--load",
        "--provenance=false",
        "--progress=plain",
        "--platform",
        "linux/amd64",
        "--build-arg",
        "HORIZON_DESKTOP=true",
        "--tag",
        "registry.example/worker:horizon-cloud",
        "--file",
        "/snapshot/.horizon/Dockerfile",
        "/snapshot/.horizon",
    ];
    assert_eq!(argv(&[]), expected);
    let layering = ["--build-arg", "HORIZON_BASE=horizon-layer:horizon-cloud-0"].map(str::to_owned);
    expected.splice(13..13, ["--build-arg", "HORIZON_BASE=horizon-layer:horizon-cloud-0"]);
    assert_eq!(argv(&layering), expected);
}

#[test]
fn intermediate_tags_stay_local_and_the_builder_driver_is_read() {
    assert_eq!(
        layer_image("horizon-cloud", 1).unwrap(),
        "horizon-layer:horizon-cloud-1"
    );
    assert!(layer_image(&"a".repeat(127), 10).is_err());
    assert_eq!(
        builder_driver("Name:          default\nDriver:        docker\n\nNodes:\n"),
        Some("docker")
    );
    assert_eq!(builder_driver("unexpected"), None);
}

#[test]
fn layers_build_in_order_on_fresh_local_bases_and_only_the_final_image_keeps_its_tag() {
    let fake = Fake::new();
    assert_eq!(fake.build(&|_| {}, &Cancellation::default()).unwrap(), "sha256:final");
    let log = fake.log();
    let builds: Vec<_> = log.iter().filter(|line| line.starts_with("buildx build")).collect();
    assert_eq!(builds.len(), 3);
    assert!(builds[0].contains("--label horizon.sibling-base.0=") && !builds[0].contains("HORIZON_BASE"));
    assert!(builds[0].contains(" --tag horizon-layer:t-0 "));
    assert!(builds[1].contains("HORIZON_BASE=horizon-layer:t-0 --label horizon.sibling-base.1="));
    assert!(builds[1].contains(" --tag horizon-layer:t-1 "));
    assert!(builds[2].contains("HORIZON_BASE=horizon-layer:t-1 --tag registry.example/worker:t "));
    assert!(
        !builds[2].contains("--label"),
        "the pushed image gets no stamp of its own"
    );
    let first_build = log.iter().position(|line| line.starts_with("buildx build")).unwrap();
    let last_build = log.iter().rposition(|line| line.starts_with("buildx build")).unwrap();
    for range in [&log[..first_build], &log[last_build..]] {
        assert!(range.iter().any(|line| line == REMOVE), "{log:?}");
        assert!(range.iter().any(|line| line == VERIFY), "{log:?}");
    }
    assert!(!log.iter().any(|line| line.contains("rm registry.example")));
}

#[test]
fn a_failed_layer_still_removes_every_intermediate_tag() {
    let fake = Fake {
        fail_tag: LOCAL[1],
        ..Fake::new()
    };
    let error = fake.build(&|_| {}, &Cancellation::default()).unwrap_err();
    assert!(matches!(error, Error::Command("image build")), "{error:?}");
    let log = fake.log();
    let failed = log
        .iter()
        .position(|line| line.contains("--tag horizon-layer:t-1 "))
        .unwrap();
    assert!(!log.iter().any(|line| line.contains("--tag registry.example/worker:t ")));
    assert!(log[failed..].iter().any(|line| line == REMOVE));
    assert!(log[failed..].iter().any(|line| line == VERIFY));
}

#[test]
fn another_buildx_driver_is_refused_before_anything_is_built() {
    let fake = Fake {
        driver: "docker-container",
        ..Fake::new()
    };
    let error = fake.build(&|_| {}, &Cancellation::default()).unwrap_err();
    assert!(error.to_string().contains("docker buildx driver"), "{error}");
    assert_eq!(fake.log(), ["buildx inspect"]);
}

#[test]
fn a_leftover_that_cannot_be_removed_is_named_and_nothing_is_built() {
    let fake = Fake {
        stale: true,
        ..Fake::new()
    };
    let output = RefCell::new(Vec::new());
    let emit = |event| {
        if let Event::Output(line) = event {
            output.borrow_mut().push(line);
        }
    };
    let error = fake.build(&emit, &Cancellation::default()).unwrap_err();
    assert!(error.to_string().contains("previous attempt"), "{error}");
    assert!(!fake.log().iter().any(|line| line.starts_with("buildx build")));
    assert!(
        output
            .borrow()
            .iter()
            .any(|line| line == "Intermediate sibling images remain: horizon-layer:t-0")
    );
}

#[test]
fn a_sibling_that_ignores_its_base_is_refused_by_name() {
    let fake = Fake {
        unrelated: true,
        ..Fake::new()
    };
    match fake.build(&|_| {}, &Cancellation::default()) {
        Err(Error::Sibling(SiblingError::Base(alias))) => assert_eq!(alias, "one"),
        other => panic!("expected a base refusal, got {other:?}"),
    }
    assert!(fake.log().iter().rev().take(2).any(|line| line == VERIFY));
}

#[test]
fn cancellation_stays_the_outcome_when_cleanup_also_fails() {
    let fake = Fake {
        block: true,
        ..Fake::new()
    };
    let cancel = Cancellation::default();
    let output = RefCell::new(Vec::new());
    let emit = |event| {
        if let Event::Output(line) = event {
            output.borrow_mut().push(line);
        }
    };
    let building = fake.root.path().join("building");
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !building.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            cancel.cancel();
        });
        fake.build(&emit, &cancel)
    });
    assert!(
        matches!(result, Err(Error::Provider(CloudError::Cancelled))),
        "{result:?}"
    );
    let output = output.borrow();
    assert!(
        output
            .iter()
            .any(|line| line == "Intermediate sibling images remain: horizon-layer:t-0")
    );
    assert!(
        output
            .iter()
            .any(|line| line.contains("Intermediate sibling image cleanup failed"))
    );
}

/// Needs a local image with `/bin/sh`, for example `HORIZON_TEST_DOCKER_BASE=alpine:3`.
#[test]
#[ignore = "requires a local Docker daemon with the docker buildx driver and HORIZON_TEST_DOCKER_BASE"]
fn layers_build_on_the_local_image_before_them_and_leave_only_the_final_tag() {
    let base = std::env::var("HORIZON_TEST_DOCKER_BASE").unwrap();
    let root = tempfile::tempdir().unwrap();
    let write = |name: &str, dockerfile: &str| {
        let path = root.path().join(name);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("Dockerfile"), dockerfile).unwrap();
        Recipe {
            dockerfile: path.join("Dockerfile"),
            context: path,
            platform: "linux/amd64".into(),
        }
    };
    let primary = write("primary", &format!("FROM {base}\nRUN echo primary > /layers\n"));
    let layer = |name: &str| format!("ARG HORIZON_BASE={base}\nFROM ${{HORIZON_BASE}}\nRUN echo {name} >> /layers\n");
    let layers = [
        ("one", write("one", &layer("one"))),
        ("two", write("two", &layer("two"))),
    ];
    let cancel = Cancellation::default();
    let runner = runner(&cancel, &|_| {});
    let builder = Builder {
        docker: || Command::new("docker"),
        runner: &runner,
    };
    let tag = format!("horizon-test-{}", new_id());
    let image = format!("horizon-layer-test:{tag}");
    let built = builder.build_layers(&image, &tag, &primary, &layers, &[]).unwrap();
    let run = |args: &[&str]| String::from_utf8(Command::new("docker").args(args).output().unwrap().stdout).unwrap();
    let layered = run(&["run", "--rm", &image, "cat", "/layers"]);
    let local = run(&[
        "image",
        "ls",
        "--quiet",
        "--filter",
        &format!("reference=horizon-layer:{tag}-*"),
    ]);
    run(&["image", "rm", &image]);
    assert!(built.starts_with("sha256:"));
    assert_eq!(layered, "primary\none\ntwo\n");
    assert!(local.trim().is_empty(), "intermediate layers remain: {local}");
}

/// Needs a local image, for example `HORIZON_TEST_DOCKER_BASE=alpine:3`.
#[test]
#[ignore = "requires a local Docker daemon with the docker buildx driver and HORIZON_TEST_DOCKER_BASE"]
fn a_real_sibling_that_ignores_a_metadata_only_base_is_refused() {
    let base = std::env::var("HORIZON_TEST_DOCKER_BASE").unwrap();
    let root = tempfile::tempdir().unwrap();
    let write = |name: &str, dockerfile: &str| {
        let path = root.path().join(name);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("Dockerfile"), dockerfile).unwrap();
        Recipe {
            dockerfile: path.join("Dockerfile"),
            context: path,
            platform: "linux/amd64".into(),
        }
    };
    let primary = write("primary", &format!("FROM {base}\nENV REQUIRED=1\n"));
    let layers = [("stray", write("stray", &format!("FROM {base}\n")))];
    let cancel = Cancellation::default();
    let runner = runner(&cancel, &|_| {});
    let builder = Builder {
        docker: || Command::new("docker"),
        runner: &runner,
    };
    let tag = format!("horizon-test-{}", new_id());
    let image = format!("horizon-layer-test:{tag}");
    let refused = builder.build_layers(&image, &tag, &primary, &layers, &[]);
    let run = |args: &[&str]| String::from_utf8(Command::new("docker").args(args).output().unwrap().stdout).unwrap();
    let local = run(&[
        "image",
        "ls",
        "--quiet",
        "--filter",
        &format!("reference=horizon-layer:{tag}-*"),
    ]);
    run(&["image", "rm", &image]);
    assert!(
        matches!(&refused, Err(Error::Sibling(SiblingError::Base(alias))) if alias == "stray"),
        "{refused:?}"
    );
    assert!(local.trim().is_empty(), "intermediate layers remain: {local}");
}

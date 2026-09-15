use super::*;
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn executable(path: &Path, content: &str) {
    fs::write(path, content).expect("script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("mode");
}

fn run(root: &Path, script: &str, mode: &str) -> std::process::Output {
    Command::new("bash")
        .args(["-c", script])
        .env("PATH", format!("{}:/usr/bin:/bin", root.join("bin").display()))
        .env("FIXTURE_ROOT", root)
        .env("FIXTURE_MODE", mode)
        .output()
        .expect("bounded fixture script")
}

const DOCKER: &str = r"#!/usr/bin/python3
import json,os,pathlib,signal,sys,time
root=pathlib.Path(os.environ['FIXTURE_ROOT']);mode=os.environ['FIXTURE_MODE'];args=sys.argv[1:]
state=root/'container'
with (root/'calls').open('a') as out:out.write(json.dumps(args)+'\n')
if args[:2]==['container','ls']:
 if state.exists(): print('a'*64)
elif args[0]=='create':
 assert args[args.index('--name')+1].startswith('horizon-runtime-probe-')
 assert args[args.index('--label')+1].startswith('io.horizon.runtime-probe=')
 state.write_text('owned')
 if mode=='create-response-lost':sys.exit(1)
 print('a'*64)
elif args[0]=='start':
 if mode=='hang':
  signal.signal(signal.SIGTERM,signal.SIG_IGN)
  while True:time.sleep(.1)
 if mode=='failed-exit':sys.exit(1)
 print(json.dumps({'passed':mode not in ['false-receipt']}))
elif args[0]=='rm':
 assert args==['rm','--force','a'*64]
 if mode=='cleanup-failed':sys.exit(1)
 state.unlink()
else:sys.exit(90)
";

#[test]
fn probes_remove_owned_containers_on_failure_timeout_and_lost_create_reply() {
    for mode in [
        "passed",
        "false-receipt",
        "failed-exit",
        "hang",
        "create-response-lost",
        "cleanup-failed",
    ] {
        let directory = tempfile::tempdir().expect("fixture");
        let root = directory.path();
        fs::create_dir(root.join("bin")).expect("bin");
        executable(&root.join("bin/docker"), DOCKER);
        let qualification = AzureContainerRuntime::WorkspaceSandboxV1
            .qualify("synthetic.azurecr.io/worker@sha256:aaaa")
            .replace("--kill-after=5s 60", "--kill-after=0.2s 0.2s");
        let script = format!("set -euo pipefail\n{qualification}\nprintf ready > \"$FIXTURE_ROOT/ready\"\n");
        let output = run(root, &script, mode);
        assert_eq!(
            output.status.success(),
            mode == "passed",
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(root.join("ready").exists(), mode == "passed", "{mode}");
        assert_eq!(root.join("container").exists(), mode == "cleanup-failed", "{mode}");
        let calls = fs::read_to_string(root.join("calls")).expect("calls");
        assert!(calls.contains("io.horizon.runtime-probe="));
        assert!(!calls.contains("horizon-worker\""));
        if mode == "passed" {
            assert!(calls.contains("0:0") && calls.contains("1000:1000"));
        }
    }
}

#[test]
fn daemon_preflight_rejects_missing_enforcement_policy_drift_and_unsupported_runtime() {
    for mode in [
        "passed",
        "apparmor-disabled",
        "seccomp-missing",
        "profile-load-failed",
        "policy-drift",
        "wrong-version",
    ] {
        let directory = tempfile::tempdir().expect("fixture");
        let root = directory.path();
        for name in [
            "bin",
            "etc/apparmor.d",
            "etc/horizon",
            "sys/module/apparmor/parameters",
            "sys/kernel/security/apparmor",
            "proc/sys/kernel/seccomp",
        ] {
            fs::create_dir_all(root.join(name)).expect("directory");
        }
        fs::write(
            root.join("etc/apparmor.d/horizon-workspace-sandbox-v1"),
            include_bytes!("../workspace-v1.apparmor"),
        )
        .expect("apparmor");
        fs::write(
            root.join("etc/horizon/worker-seccomp-v1.json"),
            include_bytes!("../workspace-v1.seccomp.json"),
        )
        .expect("seccomp");
        fs::write(
            root.join("sys/module/apparmor/parameters/enabled"),
            if mode == "apparmor-disabled" { "N\n" } else { "Y\n" },
        )
        .expect("enabled");
        fs::write(
            root.join("proc/sys/kernel/seccomp/actions_avail"),
            if mode == "seccomp-missing" {
                ""
            } else {
                "kill_process errno allow\n"
            },
        )
        .expect("actions");
        if mode == "policy-drift" {
            fs::write(root.join("etc/horizon/worker-seccomp-v1.json"), "{}").expect("drift");
        }
        executable(
            &root.join("bin/dockerd"),
            "#!/bin/bash\nif [ \"$FIXTURE_MODE\" = wrong-version ]; then echo 'Docker version 29.2.0, build fixture'; else echo 'Docker version 29.1.3, build fixture'; fi\n",
        );
        executable(
            &root.join("bin/apparmor_parser"),
            "#!/bin/bash\n[ \"$FIXTURE_MODE\" != profile-load-failed ] || exit 1\nprintf '%s\\n' 'horizon-workspace-sandbox-v1 (enforce)' > \"$FIXTURE_ROOT/sys/kernel/security/apparmor/profiles\"\n",
        );
        let mut script = AzureContainerRuntime::preflight();
        for prefix in ["/etc/", "/sys/module/", "/sys/kernel/security/", "/proc/sys/kernel/"] {
            script = script.replace(prefix, &format!("{}{prefix}", root.display()));
        }
        script.push_str("\nprintf ready > \"$FIXTURE_ROOT/ready\"\n");
        let output = run(root, &script, mode);
        assert_eq!(
            output.status.success(),
            mode == "passed",
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(root.join("ready").exists(), mode == "passed", "{mode}");
    }
}

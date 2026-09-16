use std::path::PathBuf;
use std::process::Command;

use horizon_browser_control::BrowserRuntimePaths;
use horizon_browser_control::manifest;
use horizon_browser_control::paths::{
    RUNTIME_ROOT_ENV, RuntimePathError, configure_runtime_root, default_horizon_root, initialize_from_environment,
};

#[test]
fn independent_processes_keep_runtime_roots_isolated() {
    let directory = tempfile::tempdir().expect("temporary directory");
    for mode in ["configured", "late", "empty", "race"] {
        for name in ["first", "second"] {
            let root = directory.path().join(format!("{mode}-{name}"));
            std::fs::create_dir(&root).expect("root directory");
            let output = Command::new(std::env::current_exe().expect("test executable"))
                .args(["--exact", "runtime_root_child", "--nocapture"])
                .env("BROWSER_PATH_TEST_MODE", mode)
                .env(RUNTIME_ROOT_ENV, if mode == "empty" { "" } else { "relative-runtime" })
                .env("HOME", &root)
                .current_dir(&root)
                .output()
                .expect("child test");
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            assert!(!root.join(".horizon").exists(), "application home was modified");
            assert_eq!(
                root.join("relative-runtime/runtime/browsers/proof").exists(),
                mode == "configured"
            );
        }
    }
}

#[test]
fn runtime_root_child() {
    let Ok(mode) = std::env::var("BROWSER_PATH_TEST_MODE") else {
        return;
    };
    let home = std::env::current_dir().expect("working directory");
    match mode.as_str() {
        "configured" => {
            initialize_from_environment().expect("configure relative root");
            let root = home.join("relative-runtime");
            assert_eq!(BrowserRuntimePaths::resolve().root(), root);
            assert_eq!(default_horizon_root(), home.join(".horizon"));
            configure_runtime_root(&root).expect("same absolute root is idempotent");
            assert!(matches!(
                configure_runtime_root(home.join("other")),
                Err(RuntimePathError::AlreadyInUse)
            ));
            std::env::set_current_dir(home.parent().expect("parent directory")).expect("change cwd");
            assert_eq!(BrowserRuntimePaths::resolve().root(), root);
            assert_eq!(manifest::default_manifest_dir(), root.join("runtime/browsers"));
            assert_eq!(
                BrowserRuntimePaths::resolve().browser_results_dir(),
                root.join("runtime/browser-results")
            );
            assert_eq!(
                BrowserRuntimePaths::resolve().browser_audit_dir(),
                root.join("audit/browsers")
            );
            std::fs::create_dir_all(manifest::default_manifest_dir()).expect("manifest directory");
            std::fs::write(manifest::default_manifest_dir().join("proof"), b"isolated").expect("proof");
        }
        "late" => {
            assert_eq!(BrowserRuntimePaths::resolve().root(), home.join(".horizon"));
            assert!(matches!(
                initialize_from_environment(),
                Err(RuntimePathError::AlreadyInUse)
            ));
            assert_eq!(BrowserRuntimePaths::resolve().root(), home.join(".horizon"));
        }
        "empty" => {
            assert!(matches!(
                initialize_from_environment(),
                Err(RuntimePathError::EmptyRoot)
            ));
        }
        "race" => {
            let roots = [home.join("a"), home.join("b")];
            let threads: Vec<_> = (0..16)
                .map(|index| {
                    let root = roots[index % 2].clone();
                    std::thread::spawn(move || (root.clone(), configure_runtime_root(root).is_ok()))
                })
                .collect();
            let outcomes: Vec<(PathBuf, bool)> = threads
                .into_iter()
                .map(|thread| thread.join().expect("thread"))
                .collect();
            let selected = BrowserRuntimePaths::resolve();
            for (root, success) in outcomes {
                assert_eq!(success, root == selected.root());
            }
        }
        _ => panic!("unknown test mode"),
    }
}

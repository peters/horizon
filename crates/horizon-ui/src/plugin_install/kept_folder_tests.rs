//! The plugin folder of a Horizon whose agent program runs on in a holder.
//!
//! The program was started with the path of that folder, so the folder must stay
//! after that Horizon ended. The lane runs the functions that the application runs
//! at its end and at its next start.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use horizon_core::browser::manifest::{HOST_INSTANCE_ENV, host_instance};
use horizon_core::{HorizonHome, panel_holder};

use super::{
    AgentPluginHostLease, held_agent_plugin_lease, install_agent_plugins_for, release_held_agent_plugin_host,
    work_hooks,
};

/// The id of a real panel, and the identity of the Horizon that started its program.
const HELD_PANEL: &str = "7c2b9e14-3a6d-4f08-b1c5-9e0d2a4f6b83";
const FIRST_HORIZON: &str = "1f4e8a02-6c9b-4d37-a5e1-0b7c3d9f2e64";

/// The launch of an agent stand-in as a Claude panel of `FIRST_HORIZON` gets it:
/// its arguments name the plugin folder, and its environment names that Horizon.
fn launch_that_names(plugin: &std::path::Path) -> panel_holder::Launch {
    panel_holder::Launch {
        local_id: HELD_PANEL.to_owned(),
        program: "/bin/sh".to_owned(),
        args: ["-c", "exec cat", "claude", "--plugin-dir"]
            .into_iter()
            .map(str::to_owned)
            .chain([plugin.display().to_string()])
            .collect(),
        cwd: Some(PathBuf::from("/tmp")),
        env: HashMap::from([(HOST_INSTANCE_ENV.to_owned(), FIRST_HORIZON.to_owned())]),
        rows: 24,
        cols: 80,
        cell_width: 8,
        cell_height: 17,
    }
}

/// A home in a folder of a short path, because the path of a socket is bounded.
fn short_home() -> (tempfile::TempDir, HorizonHome) {
    let folder = tempfile::Builder::new()
        .prefix("hz-kept")
        .tempdir_in("/tmp")
        .expect("a folder for a home");
    let home = HorizonHome::from_root(folder.path().join(".horizon"));
    (folder, home)
}

/// Asks `read` until it answers, for a bounded time. Another test of this process
/// that starts a program holds a copy of every lock of the process until that
/// program replaces the copy, so a lock that is free can look taken for a moment.
fn soon<T>(mut read: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match read() {
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            answer => return answer,
        }
    }
}

#[test]
fn the_plugin_folder_of_a_held_program_stays_until_its_panel_closes() {
    // This process has its own identity, as a Horizon that takes no other one.
    let _ = host_instance();
    let (folder, home) = short_home();
    let first_dir = home.agent_plugin_host_dir(FIRST_HORIZON);
    let held = home.agent_plugin_host_held_path(FIRST_HORIZON);
    let plugin = home.agent_work_plugin_dir_for_host(FIRST_HORIZON);
    let hooks = plugin.join("hooks/hooks.json");

    // The first Horizon starts: it takes its lease and writes its plugin folder.
    *held_agent_plugin_lease() = Some(AgentPluginHostLease::acquire(first_dir.clone()).expect("the first lease"));
    work_hooks::install(&plugin).expect("the plugin of the work hooks");
    panel_holder::serve_on_a_thread(&home, &launch_that_names(&plugin)).expect("the holder of the program");

    // The first Horizon ends, as `exit_after_releasing_plugins` ends it.
    release_held_agent_plugin_host();
    assert!(hooks.is_file(), "the folder of a held program outlives its Horizon");

    // The next Horizon starts, with another identity, and prunes the stale folders.
    let user_home = folder.path().join("user-home");
    std::fs::create_dir_all(&user_home).expect("a home of a person");
    let search = std::ffi::OsString::from(folder.path().join("no-programs"));
    let next = install_agent_plugins_for(&home, Some(&user_home), None, None, Some(&search)).expect("the next start");
    assert!(
        hooks.is_file(),
        "the next start keeps the folder that a held program names"
    );

    // The person closes the panel, which ends the program and its holder. The
    // start after that finds no holder for the folder and removes it.
    let ends = panel_holder::close_unshown(&home, &BTreeSet::new()).expect("the holders of the home");
    assert_eq!((ends.ended, ends.alive), (1, 0), "the close ends the program");
    drop(next);
    let removed = soon(|| {
        let later = install_agent_plugins_for(&home, Some(&user_home), None, None, Some(&search));
        (later.is_some() && !first_dir.exists()).then_some(())
    });
    assert!(removed.is_some(), "no holder names the folder, so a start removes it");
    assert!(!held.exists(), "the file of the holders goes with the folder");
}

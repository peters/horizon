use std::sync::mpsc::channel;

use horizon_core::Config;

use crate::app::test_support::{run_app_frame, test_app};

#[test]
fn the_presets_of_a_frame_list_the_hosts_that_the_worker_read() {
    // The frame path of this fix runs under the guard in strict mode: a
    // regression that reads the ssh config on the frame panics here.
    horizon_core::ui_thread::use_strict_mode();
    let (_temp, mut app) = test_app();
    // The worker answered no host yet, so the presets are the settings alone.
    run_app_frame(&egui::Context::default(), &mut app);
    assert_eq!(app.presets.len(), app.template_config.presets.len());

    let (answers, answer) = channel();
    let config = Config {
        presets: Vec::new(),
        ..Config::default()
    };
    app.template_config = config.clone();
    app.ssh_hosts = super::SshHostsRead::default();
    let _ = answers.send(Ok(vec![host("studio")]));
    app.ssh_hosts.answer = Some(answer);
    app.poll_ssh_hosts(&egui::Context::default());
    let names: Vec<&str> = app.presets.iter().map(|preset| preset.name.as_str()).collect();
    assert_eq!(names, vec!["SSH: studio"], "the read answers the presets");
    assert_eq!(config.ssh_presets(&[host("studio")]).len(), 1);
}

#[test]
fn a_read_of_the_hosts_answers_at_most_once_per_request() {
    horizon_core::ui_thread::use_strict_mode();
    let (_temp, mut app) = test_app();
    let (_answers, answer) = channel();
    app.ssh_hosts.answer = Some(answer);
    assert!(app.ssh_hosts.is_reading(), "a second request waits for the first");
}

fn host(alias: &str) -> horizon_core::DiscoveredSshHost {
    horizon_core::DiscoveredSshHost {
        alias: alias.to_owned(),
        connection: horizon_core::SshConnection {
            host: format!("{alias}.example.net"),
            ..horizon_core::SshConnection::default()
        },
    }
}

//! The route of a new browser panel, from the tool input of an agent and from
//! the action of a person to the machine that serves the page.
//!
//! The rule of `horizon_core::browser::new_browser_panel` reads the entry
//! `browser_host` of the settings, the `profile` field of a create and the engine
//! that the create named. These tests start at the fields of `browser_create` as
//! an agent sends them, let the queue of the host write the request, read that
//! request back as the host reads it, and end at `horizon_core::browser_machine`,
//! which is the decision that the spawn of a browser panel reads. No test starts
//! a browser and no test runs `ssh`.

use std::time::Duration;

use horizon_core::browser::manifest::{self, BrowserCreateRequest, RemoteCreateParameters};
use horizon_core::browser::{BackendKind, BrowserPanelState, NewBrowserPanel};
use horizon_core::{BrowserHostConfig, BrowserMachine, PanelKind, PanelOptions, browser_machine};

use super::HorizonApp;
use super::actions::support::preset_panel_options;
use super::browser_create_dispatch::browser_create_options;
use super::test_support::test_app;
use horizon_core::browser::new_browser_panel;

/// The settings entry of a browser host that runs Chromium in the profile
/// `live42`.
fn chromium_host() -> BrowserHostConfig {
    BrowserHostConfig {
        ssh_target: Some("browser-vm".to_string()),
        profile: Some("live42".to_string()),
        ..BrowserHostConfig::default()
    }
}

/// The settings entry of a browser host that runs Firefox.
fn firefox_host() -> BrowserHostConfig {
    BrowserHostConfig {
        engine: horizon_core::BrowserHostEngine::Firefox,
        ..chromium_host()
    }
}

/// An app whose settings name `host`.
fn app_with_host(host: BrowserHostConfig) -> (tempfile::TempDir, HorizonApp) {
    let (temp, mut app) = test_app();
    app.template_config.browser_host = host;
    (temp, app)
}

/// The request that a host claims for a `browser_create` call that names these
/// fields, after the queue of the agent tool wrote it.
///
/// The `profile` and `backend` fields are the two fields of the tool input that
/// decide the machine, so this is where an agent's create starts.
fn queued_create(
    temp: &tempfile::TempDir,
    profile: Option<&str>,
    backend: Option<BackendKind>,
) -> BrowserCreateRequest {
    let identity = manifest::AgentIdentity {
        actor: "horizon:agent-panel",
        host_instance: Some("host-instance"),
    };
    let request_id = manifest::enqueue_create_oriented_at(
        temp.path(),
        identity,
        RemoteCreateParameters {
            url: None,
            backend,
            target: None,
            orientation: None,
            visible: true,
            profile: profile.map(str::to_string),
        },
        Duration::from_secs(5),
    )
    .expect("the queue takes the create of this test");
    let requests = manifest::list_create_requests_at(temp.path()).expect("the host reads the queue of this test");
    requests
        .into_iter()
        .find(|request| request.request_id == request_id)
        .expect("the request of this create is one that the host claims")
}

/// The rule's answer and the options of the browser panel that `browser_create`
/// asks for, as `HorizonApp::start_requested_browser` builds them: the request
/// that the host claimed, then the rule of the host, then the fields of the
/// panel, then the login of a machine of the list.
fn agent_panel(
    app: &HorizonApp,
    temp: &tempfile::TempDir,
    profile: Option<&str>,
    backend: Option<BackendKind>,
) -> (NewBrowserPanel, PanelOptions) {
    let request = queued_create(temp, profile, backend);
    let configured = request.backend.unwrap_or(app.template_config.browser.backend);
    let panel = new_browser_panel(
        request.profile.as_deref(),
        request.backend,
        configured,
        &app.template_config.browser_host,
    )
    .expect("the create of this test");
    let mut browser_config = app.template_config.browser.clone();
    browser_config.backend = configured;
    let mut options = browser_create_options(&request, browser_config, &panel);
    app.resolve_browser_host_login(&mut options)
        .expect("the settings of this test hold the login of their host");
    (panel, options)
}

/// The options of the browser panel that a person makes: the preset of the
/// picker, the shortcut or a link of a card, and the item `Private panel` of a
/// panel menu.
fn person_panel(app: &HorizonApp, private: bool) -> PanelOptions {
    let mut options = if private {
        BrowserPanelState::inert()
            .private_panel_options()
            .expect("a private panel of an inert browser")
    } else {
        let preset = horizon_core::PresetConfig {
            name: "Browser".to_string(),
            alias: None,
            kind: PanelKind::Browser,
            command: None,
            args: Vec::new(),
            resume: horizon_core::PanelResume::Fresh,
            ssh_connection: None,
        };
        preset_panel_options(&preset, &app.template_config.browser)
    };
    app.resolve_browser_host_login(&mut options)
        .expect("the settings of this test hold the login of their host");
    options
}

fn machine_of(mut options: PanelOptions) -> BrowserMachine {
    browser_machine(&mut options).expect("the mode of a panel that this computer starts")
}

/// The two fields of a create that decide the machine reach the queue as the
/// agent wrote them: a create that names no profile and no engine names neither.
/// The rule of the host gives the profile and the engine of that host, so the
/// tool's process names no engine that the rule would read as another engine
/// than the host's.
#[test]
fn a_create_that_names_nothing_reaches_the_host_with_no_field() {
    let (temp, _app) = test_app();
    let request = queued_create(&temp, None, None);
    assert!(request.profile.is_none(), "no profile field stays no profile");
    assert!(request.backend.is_none(), "no backend field stays no engine");
}

/// A create with no `profile` is a page of the profile of the host, in the engine
/// of that host. This is the line of the rule that the queue's `default` broke:
/// the panel of an agent, and the panel of a person, both opened a browser of
/// this computer.
#[test]
fn a_create_with_no_profile_is_a_page_of_the_host_for_an_agent_and_for_a_person() {
    for host in [chromium_host(), firefox_host()] {
        let (temp, app) = app_with_host(host);
        let (panel, agent) = agent_panel(&app, &temp, None, None);
        for options in [agent, person_panel(&app, false)] {
            assert_eq!(
                machine_of(options),
                BrowserMachine::Host {
                    ssh_target: "browser-vm".to_string(),
                    profile: "live42".to_string(),
                    private: false,
                },
                "a create with no profile is a page of the profile of the host"
            );
        }
        assert!(panel.note.is_none(), "a page of the host needs no note");
    }
}

/// A create with no `profile` that names another engine is a panel of this
/// computer in that engine, in the shared profile `default` of this computer: it
/// is how a person checks a page in another engine.
#[test]
fn a_create_with_no_profile_that_names_another_engine_is_a_panel_of_this_computer() {
    let (temp, app) = app_with_host(chromium_host());
    for backend in [BackendKind::FirefoxBidi, BackendKind::SafariWebDriver] {
        let (panel, options) = agent_panel(&app, &temp, None, Some(backend));
        let note = panel.note.expect("the answer says where the page runs");
        assert!(note.contains("this computer"), "{note}");
        let engine = options
            .browser_config
            .as_ref()
            .map(|config| config.backend)
            .expect("a browser panel holds its configuration");
        assert_eq!(engine, backend, "the panel opens in the engine that the create named");
        assert_eq!(
            machine_of(options),
            BrowserMachine::ThisComputer {
                backend: engine,
                profile: Some(horizon_browser::DEFAULT_PROFILE.to_string()),
                private: false,
            },
            "an engine that the host does not run opens a page of this computer"
        );
    }
}

/// `profile: "private"` is a private page of the host, in the engine of the host.
#[test]
fn a_private_create_is_a_private_page_of_the_host() {
    for host in [chromium_host(), firefox_host()] {
        let (temp, app) = app_with_host(host);
        let (panel, agent) = agent_panel(&app, &temp, Some(horizon_browser::PRIVATE_PROFILE), None);
        for options in [agent, person_panel(&app, true)] {
            assert_eq!(
                machine_of(options),
                BrowserMachine::Host {
                    ssh_target: "browser-vm".to_string(),
                    profile: "live42".to_string(),
                    private: true,
                },
                "a private page is a page of an isolated context of the browser of the host"
            );
        }
        assert!(panel.note.is_none(), "a page of the host needs no note");
    }
}

/// A private create that names another engine is a private panel of this
/// computer.
#[test]
fn a_private_create_of_another_engine_is_a_private_panel_of_this_computer() {
    let (temp, app) = app_with_host(firefox_host());
    let (panel, options) = agent_panel(
        &app,
        &temp,
        Some(horizon_browser::PRIVATE_PROFILE),
        Some(BackendKind::ChromiumCdp),
    );
    let note = panel.note.expect("the answer says where the page runs");
    assert!(note.contains("this computer"), "{note}");
    assert_eq!(
        machine_of(options),
        BrowserMachine::ThisComputer {
            backend: BackendKind::ChromiumCdp,
            profile: None,
            private: true,
        }
    );
}

/// A create that names a profile keeps a profile of this computer, in the engine
/// of the host and in the engine that the create named.
#[test]
fn a_create_that_names_a_profile_keeps_a_profile_of_this_computer() {
    let (temp, app) = app_with_host(chromium_host());
    let (panel, options) = agent_panel(&app, &temp, Some("work"), None);
    let note = panel.note.expect("the answer says where the page runs");
    assert!(note.contains("work"), "{note}");
    assert!(note.contains("this computer"), "{note}");
    assert_eq!(
        machine_of(options),
        BrowserMachine::ThisComputer {
            backend: BackendKind::ChromiumCdp,
            profile: Some("work".to_string()),
            private: false,
        }
    );
    let (_panel, options) = agent_panel(&app, &temp, Some("work"), Some(BackendKind::FirefoxBidi));
    assert_eq!(
        machine_of(options),
        BrowserMachine::ThisComputer {
            backend: BackendKind::FirefoxBidi,
            profile: Some("work".to_string()),
            private: false,
        }
    );
}

/// The shared profile `default` is a name: a create that writes it asks for the
/// profile of this computer, and a create that writes no field asks for the
/// profile of the host.
#[test]
fn a_create_that_names_the_shared_default_keeps_it_on_this_computer() {
    let (temp, app) = app_with_host(firefox_host());
    let configured = app.template_config.browser.backend;
    let (panel, options) = agent_panel(&app, &temp, Some(horizon_browser::DEFAULT_PROFILE), None);
    assert_eq!(
        panel.reported_profile(),
        Some(horizon_browser::DEFAULT_PROFILE),
        "the page joins the shared default of this computer"
    );
    let note = panel.note.expect("the answer says where the page runs");
    assert!(note.contains(horizon_browser::DEFAULT_PROFILE), "{note}");
    assert!(note.contains("this computer"), "{note}");
    assert_eq!(
        machine_of(options),
        BrowserMachine::ThisComputer {
            backend: configured,
            profile: Some(horizon_browser::DEFAULT_PROFILE.to_string()),
            private: false,
        },
        "a create that writes the name default names a profile of this computer"
    );
}

/// Without a browser host every create is a page of this computer, as it was
/// before the entry existed.
#[test]
fn without_a_browser_host_every_create_is_a_page_of_this_computer() {
    for host in [
        BrowserHostConfig::default(),
        BrowserHostConfig {
            profile: Some("live42".to_string()),
            ..BrowserHostConfig::default()
        },
    ] {
        let (temp, app) = app_with_host(host);
        for options in [
            agent_panel(&app, &temp, None, None).1,
            agent_panel(&app, &temp, Some(horizon_browser::PRIVATE_PROFILE), None).1,
            agent_panel(&app, &temp, Some("work"), None).1,
            person_panel(&app, false),
            person_panel(&app, true),
        ] {
            assert!(
                matches!(machine_of(options), BrowserMachine::ThisComputer { .. }),
                "a settings entry that names no host keeps the browser of this computer"
            );
        }
    }
}

/// The answer of a create that opens a page of this computer says where it runs,
/// and a page of the host needs no note.
#[test]
fn the_answer_of_a_create_names_the_machine_of_a_page_that_is_not_on_the_host() {
    let (_temp, app) = app_with_host(chromium_host());
    let configured = app.template_config.browser.backend;
    let entry = app.template_config.browser_host.clone();

    let on_host = new_browser_panel(None, None, configured, &entry).expect("a page of the host");
    assert!(on_host.note.is_none(), "a page of the host needs no note");

    let other_engine =
        new_browser_panel(None, Some(BackendKind::FirefoxBidi), configured, &entry).expect("a page of another engine");
    let note = other_engine.note.expect("the answer says where the page runs");
    assert!(note.contains("this computer"), "{note}");
    assert!(note.contains("browser host"), "{note}");

    let named_profile = new_browser_panel(Some("work"), None, configured, &entry).expect("a page of one profile");
    let note = named_profile.note.expect("the answer says where the page runs");
    assert!(note.contains("work"), "{note}");
    assert!(note.contains("this computer"), "{note}");
}

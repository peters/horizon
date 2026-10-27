//! The Browser VM section: the cost it shows, the presses it offers, the refusals it
//! renders as text, and the requests it hands to the host bridge.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Context, RawInput};
use horizon_core::Config;
use horizon_core::StartupDecision;
use horizon_core::browser::manifest::provider_usage::{BrowserVmAction, BrowserVmRequest};
use horizon_core::cloud_runtime::browser_vm::{Phase, Status};
use tempfile::TempDir;

use super::app::device_connection;
use super::{Action, Panel, hourly_line, phase_label};
use crate::app::HorizonApp;
use crate::app::browser_vm_requests::{DeviceRoute, run_settings};
use crate::app::settings::SettingsTab;
use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};
use crate::test_egui::DiscardTextures;

const IMAGE: &str =
    "ghcr.io/example/browser-vm@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f";

fn app() -> (TempDir, Context, HorizonApp) {
    test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(horizon_core::RuntimeState::default()),
    })
}

fn ready() -> Status {
    Status {
        phase: Phase::Ready,
        ssh_target: Some("horizon-browser-vm".into()),
        ssh_note: None,
        image: Some(IMAGE.into()),
        server_type: Some("ccx13".into()),
        location: Some("fsn1".into()),
        volume_gb: Some(20),
        hourly_net: Some(0.0689),
        error: None,
    }
}

fn panel(app: &HorizonApp) -> &Panel {
    &app.settings.as_ref().expect("settings open").browser_vm
}

/// The request that the fields of the section make of a rent.
fn deploy(app: &mut HorizonApp, image: &str) -> BrowserVmRequest {
    let panel = &mut app.settings.as_mut().expect("settings open").browser_vm;
    panel.image = image.to_string();
    panel.location = "fsn1".to_string();
    panel.request(BrowserVmAction::Deploy)
}

/// Polls the host bridge until every run of this window ended, because a provider run
/// finishes on its own thread after it answers.
fn settled(app: &mut HorizonApp, ctx: &Context) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.browser_create_host.provider_usage.browser_vm.running() {
        assert!(Instant::now() < deadline, "a browser VM run did not end");
        app.poll_browser_vm(ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Polls the host bridge until it answers, because every request runs on its own thread.
fn wait_for_answer(app: &mut HorizonApp, ctx: &Context) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_browser_vm(ctx);
        let answered = app
            .settings
            .as_ref()
            .is_some_and(|editor| editor.browser_vm.status.is_some() || editor.browser_vm.error.is_some());
        if answered {
            return;
        }
        assert!(Instant::now() < deadline, "the host bridge answered nothing");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn private_file(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, contents).expect("write");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).expect("mode");
}

/// A cloud root with the Hetzner settings that a rent needs, so a refusal comes from the
/// request itself and not from a missing section.
fn cloud_root(temp: &TempDir) -> PathBuf {
    let root = temp.path().join("cloud");
    std::fs::create_dir_all(&root).expect("cloud root");
    let identity = root.join("identity");
    private_file(&identity, "synthetic private key");
    private_file(&root.join("identity.pub"), KEY);
    private_file(&root.join("token"), "synthetic-hetzner-token");
    let settings = serde_json::json!({
        "runpod_key_file": "/unused", "ssh_identity_file": identity, "docker_config": "/unused",
        "cpu_flavors": [], "gpu_types": [],
        "hetzner": {"token_file": root.join("token"), "server_types": ["cx43"], "locations": ["fsn1"]},
    });
    std::fs::write(root.join("settings.json"), settings.to_string()).expect("settings");
    root
}

/// What a screen reader reads from the section, with the disabled flag of each widget,
/// after one headless pass that is tall enough to lay every card out.
fn drawn(config: &mut Config, panel: &mut Panel) -> Vec<(String, bool)> {
    let ctx = Context::default();
    ctx.enable_accesskit();
    let output = ctx
        .run_ui(
            RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 2400.0))),
                ..RawInput::default()
            },
            |ui| {
                super::render(ui, config, panel);
            },
        )
        .discard_textures();
    let update = output.platform_output.accesskit_update.expect("accesskit tree");
    update
        .nodes
        .into_iter()
        .filter_map(|(_, node)| Some((node.label().or_else(|| node.value())?.to_owned(), node.is_disabled())))
        .collect()
}

fn disabled(shapes: &[(String, bool)], label: &str) -> bool {
    shapes
        .iter()
        .find(|(text, _)| text == label)
        .unwrap_or_else(|| panic!("{label} is not drawn: {shapes:?}"))
        .1
}

/// The labels of everything one frame drew, for a check that the tab of the window reaches
/// the section.
fn drawn_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
    let mut text = Vec::new();
    for clipped in shapes {
        collect_text(&clipped.shape, &mut text);
    }
    text
}

fn collect_text(shape: &egui::Shape, text: &mut Vec<String>) {
    match shape {
        egui::Shape::Text(galley) => text.push(galley.galley.job.text.clone()),
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_text(shape, text);
            }
        }
        _ => {}
    }
}

#[test]
fn the_settings_window_draws_the_browser_vm_tab() {
    let (_temp, ctx, mut app) = app();
    app.toggle_settings();
    if let Some(editor) = app.settings.as_mut() {
        editor.active_tab = SettingsTab::BrowserVm;
    }

    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None)).discard_textures();
    let text = drawn_text(&output.shapes);

    for expected in ["Browser VM", "Rent a machine", "Hourly cost", "State volume"] {
        assert!(
            text.iter().any(|line| line == expected),
            "{expected} is missing: {text:?}"
        );
    }
}

#[test]
fn the_hourly_cost_is_the_shared_net_rate() {
    let line = hourly_line(0.0689);
    assert_eq!(
        line,
        format!("{} net", horizon_core::cloud_runtime::cost::format_rate(0.0689, "EUR")),
        "the section formats a Hetzner rate with the shared cost code"
    );
    assert!(line.contains("0.0689"), "the rate keeps the four decimals: {line}");
    assert!(line.ends_with("/h net"), "the rate is hourly and net of VAT: {line}");
}

#[test]
fn a_run_in_flight_leaves_the_start_disabled() {
    let mut panel = Panel::default();
    let request = panel
        .sync(true, true)
        .expect("the section reads the phase when it opens");
    assert_eq!(request.action, BrowserVmAction::Status);
    let offered = panel.offered();
    assert!(!offered.start, "a second provider run waits for the first");
    assert!(offered.refresh, "a read is answered while a run is in flight");
    assert!(!offered.delete && !offered.stop && !offered.resume);

    panel.absorb(BrowserVmAction::Status, Ok(ready()));
    assert!(!panel.offered().start, "a ready machine is not rented again");
    panel.sync(true, false);
    assert!(panel.offered().stop, "a ready machine can be stopped");
}

#[test]
fn a_refused_mutable_tag_shows_the_refusal_and_rents_nothing() {
    let (temp, ctx, mut app) = app();
    let root = cloud_root(&temp);
    app.cloud_prototype.use_root(Some(root.clone()));
    app.toggle_settings();
    let request = deploy(&mut app, "ghcr.io/example/browser-vm:1780");

    app.queue_settings_browser_vm(request, &ctx);
    wait_for_answer(&mut app, &ctx);
    settled(&mut app, &ctx);

    let error = panel(&app).error.as_deref().expect("the refusal is text");
    assert!(error.contains("digest"), "the refusal names the digest: {error}");
    assert!(
        !root.join("browser-vm.json").exists(),
        "a refused rent records no run and rents nothing"
    );
    assert!(
        !app.browser_create_host.provider_usage.browser_vm.running(),
        "the refused run left no job behind"
    );
}

#[test]
fn a_status_read_runs_no_provider_work() {
    // A cloud root without cloud settings. A read answers the empty record, while any run
    // that reached the admission step would refuse for the missing Hetzner section.
    let (temp, ctx, mut app) = app();
    let root = temp.path().join("cloud");
    std::fs::create_dir_all(&root).expect("cloud root");
    app.cloud_prototype.use_root(Some(root.clone()));
    app.toggle_settings();
    let request = app
        .settings
        .as_mut()
        .expect("settings open")
        .browser_vm
        .request(BrowserVmAction::Status);

    app.queue_settings_browser_vm(request, &ctx);
    wait_for_answer(&mut app, &ctx);
    settled(&mut app, &ctx);

    let panel = panel(&app);
    assert_eq!(panel.phase(), Some(Phase::Idle), "a read answers the recorded phase");
    assert!(panel.error.is_none(), "a read reached no provider: {:?}", panel.error);
    assert!(!root.join("browser-vm.json").exists(), "a read writes no record");
}

#[test]
fn a_rent_without_cloud_support_shows_the_refusal_as_text() {
    let (_temp, ctx, mut app) = app();
    app.cloud_prototype.use_root(None);
    app.toggle_settings();
    let request = deploy(&mut app, IMAGE);

    app.queue_settings_browser_vm(request, &ctx);
    settled(&mut app, &ctx);

    let error = panel(&app).error.as_deref().expect("the refusal is text");
    assert!(error.starts_with("browser_vm_unavailable"), "{error}");
    assert!(
        !app.browser_create_host.provider_usage.browser_vm.running(),
        "a refused rent starts no run"
    );
}

#[test]
fn a_ready_machine_offers_the_device_panel_and_the_settings_write() {
    let mut panel = Panel::default();
    panel.absorb(BrowserVmAction::Deploy, Ok(ready()));
    assert!(panel.offered().sign_in, "a ready machine offers the one-time sign-in");

    let route = panel.device_route().expect("the display has an SSH route");
    assert_eq!(route.host, "horizon-browser-vm");
    assert_eq!(route.user, "root");
    assert_eq!(route.vnc, "127.0.0.1:5900");

    let mut config = Config::default();
    assert_eq!(panel.use_host(&mut config), Ok(true));
    assert_eq!(
        config.browser_host.ssh_target.as_deref(),
        Some("horizon-browser-vm"),
        "the settings entry names the managed entry, so a new address keeps it"
    );
    assert_eq!(panel.use_host(&mut config), Ok(false), "the entry is written one time");
}

#[test]
fn a_run_that_a_crash_left_in_progress_is_taken_over_or_deleted() {
    // The record names a phase that no run of this window performs, because the run ended
    // with the window. A start takes a recorded rent over, and a delete removes it.
    let mut panel = Panel::default();
    panel.absorb(
        BrowserVmAction::Status,
        Ok(Status {
            phase: Phase::Deploying,
            ..ready()
        }),
    );
    let offered = panel.offered();
    assert!(offered.start, "a recorded rent is taken over by a start");
    assert!(offered.delete, "only a delete removes what a lost run rented");

    panel.absorb(
        BrowserVmAction::Status,
        Ok(Status {
            phase: Phase::Deleting,
            ..ready()
        }),
    );
    let offered = panel.offered();
    assert!(!offered.start, "a recorded delete is not a rent");
    assert!(offered.delete, "a recorded delete is finished by a delete");
}

#[test]
fn a_delete_waits_for_a_second_press() {
    let mut panel = Panel::default();
    panel.absorb(BrowserVmAction::Status, Ok(ready()));
    assert!(panel.offered().delete);
    assert!(!panel.confirms_delete);

    panel.arm_delete();
    assert!(panel.confirms_delete, "the section asks before it deletes");
    panel.ask(BrowserVmAction::Delete);
    assert!(!panel.confirms_delete, "the confirmation closes with the request");

    let actions = panel.take_actions();
    let Some(Action::Queue(request)) = actions.first() else {
        panic!("the second press queues the delete");
    };
    assert_eq!(request.action, BrowserVmAction::Delete);
    assert!(
        request.image.is_none() && request.server_type.is_none() && request.volume_gb.is_none(),
        "a delete names no machine to rent"
    );
}

#[test]
fn only_a_rent_names_the_machine() {
    let panel = Panel {
        image: "  ghcr.io/example/browser-vm@sha256:abc  ".into(),
        location: " fsn1 ".into(),
        ..Panel::default()
    };

    let rent = panel.request(BrowserVmAction::Deploy);
    assert_eq!(rent.image.as_deref(), Some("ghcr.io/example/browser-vm@sha256:abc"));
    assert_eq!(rent.location.as_deref(), Some("fsn1"));
    assert_eq!(
        rent.server_type.as_deref(),
        Some("ccx13"),
        "the default size is a ccx server"
    );
    assert_eq!(rent.volume_gb, Some(20));

    for action in [
        BrowserVmAction::Status,
        BrowserVmAction::Stop,
        BrowserVmAction::Resume,
        BrowserVmAction::Delete,
    ] {
        let request = panel.request(action);
        assert!(request.image.is_none(), "{action:?} names no machine");
        assert!(request.volume_gb.is_none(), "{action:?} names no volume");
        assert!(request.valid(), "{action:?} asks only for the recorded machine");
    }
}

#[test]
fn the_section_draws_the_phase_the_cost_and_the_sign_in_of_a_ready_machine() {
    let mut panel = Panel::default();
    panel.absorb(BrowserVmAction::Status, Ok(ready()));
    let shown = drawn(&mut Config::default(), &mut panel);

    for expected in [
        phase_label(Phase::Ready),
        hourly_line(0.0689).as_str(),
        "horizon-browser-vm",
        "ccx13",
        "fsn1",
    ] {
        assert!(
            shown.iter().any(|(text, _)| text == expected),
            "{expected} is missing: {shown:?}"
        );
    }
    assert!(
        !disabled(&shown, "Open Device panel"),
        "the sign-in is offered: {shown:?}"
    );
    assert!(
        !disabled(&shown, "Use this host for new panels"),
        "the settings write is offered: {shown:?}"
    );
}

#[test]
fn the_section_draws_a_run_in_flight_with_start_disabled() {
    let mut panel = Panel::default();
    panel.absorb(BrowserVmAction::Status, Ok(ready()));
    panel.sync(true, true);
    let shown = drawn(&mut Config::default(), &mut panel);

    assert!(disabled(&shown, "Start"), "a second run waits: {shown:?}");
    assert!(disabled(&shown, "Stop"), "only one provider run runs: {shown:?}");
    assert!(!disabled(&shown, "Refresh"), "a read is answered meanwhile: {shown:?}");
}

#[test]
fn the_section_draws_no_sign_in_before_a_machine_is_ready() {
    let mut panel = Panel::default();
    panel.absorb(
        BrowserVmAction::Status,
        Ok(Status {
            phase: Phase::Idle,
            ssh_target: None,
            image: None,
            server_type: None,
            location: None,
            volume_gb: None,
            hourly_net: None,
            error: None,
            ssh_note: None,
        }),
    );
    let shown = drawn(&mut Config::default(), &mut panel);

    assert!(!disabled(&shown, "Start"), "an idle section offers the rent: {shown:?}");
    assert!(!shown.iter().any(|(text, _)| text == "Open Device panel"), "{shown:?}");
    assert!(!shown.iter().any(|(text, _)| text == "Delete"), "{shown:?}");
}

#[test]
fn the_sign_in_panel_names_the_managed_entry_and_no_key() {
    // The entry of the ssh configuration carries the key that the deploy installed, so the
    // panel presents none of its own and ssh reads the entry.
    let route = DeviceRoute {
        host: "horizon-browser-vm".into(),
        user: "root".into(),
        vnc: "127.0.0.1:5900".into(),
    };
    let connection = device_connection(&route);
    let args = connection.to_command_args();

    assert!(!args.contains(&"-i".to_string()), "no key of its own: {args:?}");
    assert_eq!(connection.host, "horizon-browser-vm");
    assert_eq!(connection.user.as_deref(), Some("root"));
    assert_eq!(args.last().map(String::as_str), Some("root@horizon-browser-vm"));
}

#[test]
fn a_machine_without_an_entry_keeps_the_address_of_the_machine() {
    // Without an entry, or without the home that holds its configuration, Horizon names the
    // address that the provider returned, and says why the entry is missing.
    let mut panel = Panel::default();
    panel.absorb(
        BrowserVmAction::Status,
        Ok(Status {
            ssh_target: Some("root@203.0.113.10".into()),
            ssh_note: Some("the name horizon-browser-vm is taken".into()),
            ..ready()
        }),
    );

    let route = panel.device_route().expect("a ready machine has a route");
    assert_eq!(route.host, "203.0.113.10");
    assert_eq!(route.user, "root");
    let shown = drawn(&mut Config::default(), &mut panel);
    assert!(
        shown.iter().any(|(text, _)| text == "root@203.0.113.10"),
        "the section names the address it has: {shown:?}"
    );

    let mut config = Config::default();
    assert_eq!(panel.use_host(&mut config), Ok(true));
    assert_eq!(
        config.browser_host.ssh_target.as_deref(),
        Some("root@203.0.113.10"),
        "the settings entry names the route that is recorded"
    );
}

#[test]
fn the_section_shows_why_the_managed_entry_is_missing() {
    let mut panel = Panel::default();
    let note = "Horizon did not write the ssh entry horizon-browser-vm: this computer names it already";
    panel.absorb(
        BrowserVmAction::Status,
        Ok(Status {
            ssh_note: Some(note.into()),
            ..ready()
        }),
    );

    assert_eq!(panel.ssh_note(), Some(note));
    let shown = drawn(&mut Config::default(), &mut panel);
    assert!(
        shown.iter().any(|(label, _)| label.contains(note)),
        "a person reads why the entry is missing: {shown:?}"
    );
    let mut plain = Panel::default();
    plain.absorb(BrowserVmAction::Status, Ok(ready()));
    assert_eq!(plain.ssh_note(), None, "an entry of the plain case says nothing");
}

#[test]
fn absent_cloud_settings_are_the_typed_refusal_not_a_file_error() {
    let root = TempDir::new().expect("temp root");
    let reason = run_settings(root.path()).expect_err("no settings to read");
    assert!(
        reason.starts_with("browser_vm_unavailable:"),
        "a person reads why cloud support is unavailable, not a path error: {reason}"
    );
}

#[test]
fn settings_that_cannot_be_read_do_not_claim_cloud_support_is_missing() {
    let root = TempDir::new().expect("temp root");
    std::fs::write(root.path().join("settings.json"), b"{ not json").expect("broken settings");
    let reason = run_settings(root.path()).expect_err("the file is broken");
    assert!(
        reason.starts_with("browser_vm_settings_unreadable:"),
        "a broken file names itself instead of asking for a cloud setup: {reason}"
    );
}

#[test]
fn a_run_that_failed_inside_the_provider_shows_its_recorded_reason() {
    let mut panel = Panel::default();
    let reason = "The state volume is in another location";
    panel.absorb(
        BrowserVmAction::Deploy,
        Ok(Status {
            phase: Phase::Failed,
            ssh_target: None,
            error: Some(reason.into()),
            ..ready()
        }),
    );

    assert_eq!(panel.failure(), Some(reason), "a person reads why the run failed");
    let shown = drawn(&mut Config::default(), &mut panel);
    assert!(
        shown.iter().any(|(label, _)| label.contains(reason)),
        "the reason reaches the section: {shown:?}"
    );
}

#[test]
fn a_run_that_has_not_failed_shows_no_reason() {
    let mut panel = Panel::default();
    panel.absorb(BrowserVmAction::Status, Ok(ready()));
    assert_eq!(panel.failure(), None);
    let empty = Panel::default();
    assert_eq!(empty.failure(), None, "an unread section shows no reason");
}

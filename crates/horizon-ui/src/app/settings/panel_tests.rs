//! The panel of the Settings window: its width, its tab row, the rows of the
//! Server tab, and the rectangle that the canvas leaves beside it.

use horizon_core::{Config, RuntimeState, StartupDecision};

use super::panel_test_support::{
    BUTTON_LABELS, canvas_background_clip, for_each_painted, panel_background_rect, panel_texts, reserved_panel_rect,
    text_rects, text_start,
};
use super::tests::{collect_texts, ephemeral, texts_hold};
use crate::app::test_support::test_app_with_startup;

#[test]
fn the_tab_row_fits_one_line_at_the_default_panel_width() {
    use crate::test_egui::DiscardTextures;

    let labels: Vec<&str> = super::SettingsTab::ALL.iter().map(|tab| tab.label()).collect();
    // The context of a window, so the row is measured with the font that the
    // window draws and not with the default font of egui.
    let (_temp, ctx, _app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let mut needed = 0.0;
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            needed = ui.ctx().fonts_mut(|fonts| {
                super::tab_row_width(
                    |label| {
                        fonts
                            .layout_no_wrap(
                                label.to_owned(),
                                egui::FontId::proportional(super::TAB_LABEL_SIZE),
                                egui::Color32::WHITE,
                            )
                            .size()
                            .x
                    },
                    &labels,
                )
            });
        })
        .discard_textures();

    // The panel of a window of 1480 points, which is the width that the fixture
    // gives the window of a test: the tab row must fit the narrowest panel that a
    // person opens this window with.
    for viewport in [1480.0_f32, 1600.0] {
        let panel = super::settings_panel_default_width(viewport);
        let content = panel - 2.0 * f32::from(super::SETTINGS_PANEL_MARGIN_X);
        assert!(
            needed <= content,
            "the {labels:?} tabs need {needed} and the panel of a {viewport} point window gives {content}"
        );
    }
}

#[test]
fn every_tab_draws_on_one_line_at_the_default_panel_width() {
    use crate::test_egui::DiscardTextures;

    // The panel of a window of 1480 points, which is the width that the fixture
    // gives the window of a test.
    let panel = super::settings_panel_default_width(1480.0);
    let content = panel - 2.0 * f32::from(super::SETTINGS_PANEL_MARGIN_X);
    // The context of a window, so the row is drawn with the font that the window
    // draws and not with the default font of egui.
    let (_temp, ctx, _app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let mut editor = super::SettingsEditor {
        config: Config::default(),
        original: Config::default(),
        dirty: false,
        status: super::SettingsStatus::None,
        conflict: None,
        panel_rect: None,
        active_tab: super::SettingsTab::General,
        credential_inputs: super::remote_browsers::CredentialInputs::default(),
        portable_profile: super::remote_browsers::PortableProfilePanel::new(std::path::Path::new("/s.json")),
        provider_usage: super::remote_usage::UsagePanels::default(),
        secret_inputs: super::secrets::SecretInputs::default(),
        server: super::server::ServerTabState::default(),
    };
    let mut measured = (0.0_f32, 0.0_f32);
    let output = ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(content + 2.0 * f32::from(super::SETTINGS_PANEL_MARGIN_X), 600.0),
                )),
                ..egui::RawInput::default()
            },
            |ui| {
                ui.set_width(content);
                super::render_tab_bar(ui, &mut editor);
                measured = (ui.min_rect().height(), content);
            },
        )
        .discard_textures();
    let mut texts = Vec::new();
    for clipped in &output.shapes {
        collect_texts(&clipped.shape, &mut texts);
    }

    // One row of a 12 px label with 5 px of vertical padding is under 30 points; a
    // second row would be above 40.
    assert!(
        measured.0 <= 34.0,
        "the tabs stay on one line in {} points: {texts:?}",
        measured.1
    );
    for tab in [
        "General", "Keys", "Presets", "Browser", "Remote", "Secrets", "Server", "Speech",
    ] {
        assert!(texts_hold(&texts, tab), "the row draws {tab}: {texts:?}");
    }
}

/// A long value in a field must scroll and must not widen the panel: the panel
/// is a fraction of the viewport, and a wider panel covers the canvas it is
/// meant to sit beside. The widths are compared with the same panel on the same
/// machine, because the default width and the scroll bar differ by platform.
#[test]
fn a_long_text_field_does_not_widen_the_settings_panel() {
    let (_temp, ctx, mut app) = test_app_with_startup(ephemeral());
    let viewport = [1600.0, 1000.0];
    app.toggle_settings();
    if let Some(editor) = app.settings.as_mut() {
        editor.config.remote_hosts.default_workspace = "a-short-name".to_string();
    }
    let short = widest_panel_width(&ctx, &mut app, viewport);
    if let Some(editor) = app.settings.as_mut() {
        editor.config.remote_hosts.default_workspace = "a-very-long-workspace-name-".repeat(20);
    }
    let long = widest_panel_width(&ctx, &mut app, viewport);

    assert!(
        long <= short + 1.0,
        "a long line must not widen the panel: {short} wide with a short line, {long} with a long line"
    );
    assert!(
        long < viewport[0] / 2.0,
        "the panel stays below half of the 1600 pixel viewport, but it drew {long} wide"
    );
}

/// The Server tab draws the state of the link, the code and the link of an open
/// invitation, the plate of its QR code, and the three computers of the server with
/// their removal, in the panel of a window of 1480 points.
#[test]
fn the_server_tab_draws_the_invitation_the_plate_and_the_computers() {
    let (temp, ctx, mut app, _names) = app_with_stored_secrets();
    seed_server_tab(&mut app, temp.path());
    assert!(
        app.settings
            .as_ref()
            .is_some_and(|editor| editor.server.invitation.is_some()),
        "the test seeded an invitation"
    );
    let shapes = settings_frame(&ctx, &mut app, [1480.0, 900.0], super::SettingsTab::Server);
    assert!(
        app.settings
            .as_ref()
            .is_some_and(|editor| editor.server.invitation.is_some()),
        "the invitation is still open after the frames"
    );
    let mut texts = Vec::new();
    for clipped in &shapes {
        collect_texts(&clipped.shape, &mut texts);
    }
    for expected in [
        "ABCD-EFGH-JKMN",
        "Copy link",
        "Computers",
        "workshop-laptop-of-the-north-window-01",
        "workshop-laptop-of-the-south-window-02",
        "Remove",
    ] {
        assert!(
            texts_hold(&texts, expected),
            "the Server tab draws {expected}: {texts:?}"
        );
    }
    // The console of the server computer itself says what it is and offers no
    // removal: that row holds the login of the server, and a removal of it reads as
    // if it removed the server.
    assert_row_of_the_server_computer(&shapes, &texts);
    // The row that confirms a removal and the rows of the computers fit the panel
    // of this window: no word of a row lies under a button of that row.
    assert_no_word_lies_under_a_button(&shapes);
    // The plate is a run of dark rectangles on a white one, and the code of the
    // invitation has some of them.
    let mut dark = 0;
    let mut white = 0;
    for_each_painted(&shapes, &mut |shape, _painted, _clip| {
        if let egui::Shape::Rect(rect) = shape {
            if rect.fill == egui::Color32::BLACK {
                dark += 1;
            } else if rect.fill == egui::Color32::WHITE {
                white += 1;
            }
        }
    });
    assert!(white >= 1, "the plate is light in both themes");
    assert!(dark > 20, "the plate paints the modules of the code: {dark}");
    assert!(
        texts.iter().any(|text| text.ends_with("minutes left")),
        "the invitation says how long it lives: {texts:?}"
    );
}

/// The row of the server computer says that it is the server, and where the row of
/// another computer holds its Remove button this row holds that word: no control of
/// a removal is on its line.
fn assert_row_of_the_server_computer(shapes: &[egui::epaint::ClippedShape], texts: &[String]) {
    let name = text_rects(shapes, horizon_sync::SERVER_DEVICE)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the list draws the row of the server computer: {texts:?}"));
    let on_the_same_line = |rects: Vec<egui::Rect>| {
        rects
            .into_iter()
            .filter(|rect| (rect.center().y - name.center().y).abs() <= 30.0)
            .collect::<Vec<_>>()
    };
    let words = on_the_same_line(text_rects(shapes, "this server"));
    assert_eq!(
        words.len(),
        1,
        "the row of the server computer says that it is the server: {texts:?}"
    );
    for label in ["Remove", "Keep"] {
        assert!(
            on_the_same_line(text_rects(shapes, label)).is_empty(),
            "the row of the server computer offers no {label}: {texts:?}"
        );
    }
}

/// No word of the panel lies under a button of the panel: the row of a computer
/// gives its text the width that its buttons leave, and a longer line wraps under
/// itself instead of under a control.
fn assert_no_word_lies_under_a_button(shapes: &[egui::epaint::ClippedShape]) {
    let mut words = Vec::new();
    let mut buttons = Vec::new();
    for_each_painted(shapes, &mut |shape, painted, clip| {
        if let egui::Shape::Text(drawn) = shape {
            let text = drawn.galley.text();
            // A tab that is taller than its panel scrolls, and the part that the
            // scroll area cut away is not where the person reads it: the check
            // compares the part that the clip leaves.
            let visible = painted.intersect(clip);
            if visible.width() <= 0.0 || visible.height() <= 0.0 {
                return;
            }
            if BUTTON_LABELS.contains(&text) {
                buttons.push(visible);
            } else {
                words.push((text.to_owned(), visible));
            }
        }
    });
    assert!(!buttons.is_empty(), "the panel draws its buttons");
    for (text, word) in &words {
        for button in &buttons {
            assert!(
                !word.intersects(*button),
                "the word {text:?} at {word:?} lies under the button at {button:?}"
            );
        }
    }
}

/// One link whose child ends at once, one open invitation and three computers of
/// the server, so the Server tab draws its plate, its link and its list.
pub(super) fn seed_server_tab(app: &mut crate::app::HorizonApp, directory: &std::path::Path) {
    use std::sync::Arc;

    use horizon_core::secret::SecretVault;
    use horizon_core::secret_store::SecretStore;
    use horizon_sync::{DeviceLogin, DeviceRow, Invited, Supervisor, SupervisorConfig};

    let credentials = Arc::new(horizon_core::secret::CredentialVault::new(Box::new(
        SecretStore::in_memory().expect("a store in memory"),
    )));
    credentials.register(&horizon_core::browser::remote::RemoteBrowserConfig::default());
    let named = Arc::new(horizon_core::named_secrets::NamedSecretVault::new(Box::new(
        SecretStore::in_memory().expect("a store in memory"),
    )));
    let keys = horizon_core::MachineKeys::new(directory.join("server-tab-keys"));
    let vault = SecretVault::new(keys, Arc::clone(&credentials), named);
    let mut config = SupervisorConfig::for_server(
        "server-vm",
        "horizon --server-link",
        &directory.join("server-tab-link"),
        Arc::new(vault),
    )
    .expect("the settings of the link");
    config.program = "/bin/true".to_owned();
    let supervisor = Supervisor::start(config, || {}).expect("the supervisor");
    app.server_sync = Some(crate::app::server_sync::ServerSync::for_test(supervisor, credentials));

    let row = |name: &str, login: DeviceLogin, caller: bool| DeviceRow {
        name: name.to_owned(),
        created_millis: 1_760_000_000_000,
        last_seen_millis: 1_760_000_000_000,
        login,
        caller,
    };
    let editor = app.settings.as_mut().expect("the settings window");
    editor.server.invitation = Some(super::server::OpenInvitation::for_test(Invited {
        code: "ABCD-EFGH-JKMN".to_owned(),
        link: "horizon://server-computer:7000/join#ABCD-EFGH-JKMN".to_owned(),
        expires_millis: 600_000,
        now_millis: 1_759_999_999_000,
    }));
    // Three computers with the longest names that the rule of a device name
    // allows, and one removal that waits for its confirmation. The console of the
    // server computer itself is one row above them, as the list of the server
    // reports it.
    editor.server.devices = vec![
        row(horizon_sync::SERVER_DEVICE, DeviceLogin::Ssh, false),
        row("workshop-laptop-of-the-north-window-01", DeviceLogin::Ssh, true),
        row("workshop-laptop-of-the-south-window-02", DeviceLogin::Token, false),
        row("workshop-laptop-of-the-east-window-003", DeviceLogin::Token, false),
    ];
    editor.server.confirming = Some("workshop-laptop-of-the-south-window-02".to_owned());
    editor.server.devices_now_millis = 1_760_000_000_000;
}

/// The rectangle of the Settings panel, on every tab, with one stored secret and
/// a name of 64 characters: the panel keeps one rectangle, every widget that the
/// panel paints stays inside it, and the canvas ends where the panel begins.
///
/// An overflowing tab asks for more than the panel has, and egui then moves the
/// left edge of the panel while the panel itself keeps painting from the edge it
/// reserved. The canvas takes the rectangle that the panel leaves, so a tab that
/// outgrows the panel must not move the canvas into it.
#[test]
fn the_secrets_tab_fits_the_panel_that_ends_the_canvas() {
    let viewport = [1480.0, 900.0];
    let (temp, ctx, mut app, _names) = app_with_stored_secrets();
    // The Server tab is one more case, with the QR plate of an open invitation and
    // three computers of the server.
    seed_server_tab(&mut app, temp.path());

    let mut panels = Vec::new();
    for tab in super::SettingsTab::ALL {
        panels.push((tab.label(), assert_tab_fits_the_panel(&ctx, &mut app, viewport, *tab)));
    }

    // The panel is one rectangle, whatever the tab and whatever the content.
    let (first_tab, first) = panels.first().copied().expect("six tabs");
    for (tab, panel) in &panels {
        assert!(
            same_rect(*panel, first),
            "the panel is {first:?} on {first_tab} and {panel:?} on {tab}"
        );
    }
    assert!(
        first.width() < viewport[0] / 2.0,
        "the panel stays below half of the viewport: {panels:?}"
    );

    assert_open_rows_fit_the_panel(&ctx, &mut app, viewport, first);
}

/// One application frame that shows the Settings panel, with two named secrets
/// that this computer holds a value for: `HZ_DEMO_TOKEN`, and a name of 64
/// characters.
pub(super) fn app_with_stored_secrets() -> (tempfile::TempDir, egui::Context, crate::app::HorizonApp, [String; 2]) {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use horizon_core::browser::remote::{
        ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAdapterKind,
        RemoteAuthentication, RemoteProviderProfile, RemoteSessionLimits,
    };
    use horizon_core::named_secrets::{MAX_NAME_CHARS, NamedSecretState, NamedSecretVault, NamedSecretWorkbench};
    use horizon_core::remote_browser_credential::SharedCredentialStore;
    use horizon_core::secret_store::SecretStore;

    let long_name = "A".repeat(MAX_NAME_CHARS);
    let demo_name = "HZ_DEMO_TOKEN".to_string();
    let values = Arc::new(Mutex::new(SecretStore::in_memory().expect("a store in memory")));
    for name in [long_name.as_str(), demo_name.as_str()] {
        values
            .lock()
            .expect("the store")
            .write(&format!("named/{name}"), b"a-value-of-this-computer")
            .expect("the value");
    }
    let vault = Arc::new(NamedSecretVault::new(Box::new(SharedCredentialStore::new(Arc::clone(
        &values,
    )))));
    let mut config = Config::default();
    config.secrets.names = vec![long_name.clone(), demo_name.clone()];
    // A remote provider whose reference is long, so the Remote tab has a row
    // that asks for more width than the panel has.
    let reference = CredentialReference::from("a-reference-name-of-a-credential-that-is-long");
    config.browser.remote.providers.insert(
        "a-provider-with-a-long-name".to_string(),
        RemoteProviderProfile {
            adapter: RemoteAdapterKind::Webdriver,
            endpoint: ControlEndpoint::parse("https://grid.example.net/wd/hub").expect("the endpoint"),
            authentication: RemoteAuthentication::Bearer {
                token_ref: reference.clone(),
            },
            credential_bindings: BTreeMap::from([(
                reference,
                CredentialBinding {
                    store: CredentialStoreKind::Session,
                    slot: None,
                },
            )]),
            limits: RemoteSessionLimits::default(),
        },
    );
    let names = config.secrets.secret_names();
    vault.set_names(config.secrets.names.clone());

    let (temp, ctx, mut app) = crate::app::test_support::test_app_with_config_and_startup(&config, ephemeral());
    app.toggle_settings();
    let viewport = [1480.0, 900.0];
    // The startup read of the machine list hands its own vault over, so the test
    // waits for that answer before it takes the workbench of its store.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.machine_load_rx.is_some() && std::time::Instant::now() < deadline {
        run_settings_frame(&ctx, &mut app, viewport);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        app.machine_load_rx.is_none(),
        "the startup read of the machine list answered"
    );
    app.named_secrets = NamedSecretWorkbench::new(Some(vault), &names);
    while app.named_secrets.is_busy() && std::time::Instant::now() < deadline {
        app.named_secrets.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for name in [long_name.as_str(), demo_name.as_str()] {
        assert!(
            matches!(app.named_secrets.state_of(name), NamedSecretState::Present),
            "the test holds a stored secret for {name}"
        );
    }
    (temp, ctx, app, [long_name, demo_name])
}

/// Three frames of `app`, after the panel settled, and the shapes of the last
/// one. The panel is selected on `tab` first.
pub(super) fn settings_frame(
    ctx: &egui::Context,
    app: &mut crate::app::HorizonApp,
    viewport: [f32; 2],
    tab: super::SettingsTab,
) -> Vec<egui::epaint::ClippedShape> {
    if let Some(editor) = app.settings.as_mut() {
        editor.active_tab = tab;
    }
    let mut output = None;
    for _ in 0..3 {
        output = Some(run_settings_frame(ctx, app, viewport));
    }
    output.expect("three frames").shapes
}

fn run_settings_frame(ctx: &egui::Context, app: &mut crate::app::HorizonApp, viewport: [f32; 2]) -> egui::FullOutput {
    // The panel stands at the right edge of the window, and the tests measure it
    // there, with no dock beside it.
    app.orchestrator_dock = crate::app::orchestrator_dock::OrchestratorDock::new(false);
    crate::app::test_support::run_app_frame_with_input(ctx, app, crate::app::test_support::raw_input(viewport, None))
}

/// Checks one tab against the panel and the canvas, and answers the rectangle of
/// the panel that it paints.
pub(super) fn assert_tab_fits_the_panel(
    ctx: &egui::Context,
    app: &mut crate::app::HorizonApp,
    viewport: [f32; 2],
    tab: super::SettingsTab,
) -> egui::Rect {
    let shapes = settings_frame(ctx, app, viewport, tab);
    let panel = panel_background_rect(&shapes, reserved_panel_rect(app));
    let canvas = app.canvas_rect(ctx);
    let label = tab.label();

    // No widget of the panel draws beside the panel. A field or a scrolling card
    // cuts its own content, so a shape that its own clip cuts is not a shape that
    // the person sees outside the panel.
    for_each_painted(&shapes, &mut |_, painted, clip| {
        if !panel.expand(1.0).contains_rect(clip) || !clip.contains_rect(painted) {
            return;
        }
        assert!(
            painted.min.x >= panel.min.x - 1.0 && painted.max.x <= panel.max.x + 1.0,
            "the {label} tab draws {painted:?} beside the panel {panel:?}"
        );
    });

    // The content of the tab asks for no more width than the window gives the
    // panel: the panel opens on this width, and its content must fit it.
    let given = super::settings_panel_default_width(viewport[0]);
    assert!(
        panel.width() <= given + 2.0,
        "the {label} tab asks for {panel:?} and the window gives the panel {given}"
    );

    assert_state_keeps_its_place(&shapes, panel, tab);

    // The canvas rectangle and the panel rectangle do not intersect, and the
    // canvas paints only up to the left edge of the panel.
    assert!(
        !canvas.intersects(panel),
        "the {label} tab leaves the canvas {canvas:?} over the panel {panel:?}"
    );
    let canvas_clip = canvas_background_clip(&shapes, viewport);
    assert!(
        canvas.max.x <= panel.min.x + 1.0 && canvas_clip.max.x <= panel.min.x + 1.0,
        "the {label} tab ends the canvas at {} and paints it up to {}, and the panel starts at {}",
        canvas.max.x,
        canvas_clip.max.x,
        panel.min.x
    );

    assert_first_characters_show(&shapes, panel, canvas, label, tab);
    panel
}

/// The state of a secret keeps its own place in its row: it does not run under
/// the two buttons of that row.
fn assert_state_keeps_its_place(shapes: &[egui::epaint::ClippedShape], panel: egui::Rect, tab: super::SettingsTab) {
    if tab != super::SettingsTab::Secrets {
        return;
    }
    let states = text_rects(shapes, "Stored on this computer");
    let buttons: Vec<_> = ["Replace value", "Delete"]
        .into_iter()
        .flat_map(|label| text_rects(shapes, label))
        .collect();
    assert_eq!(states.len(), 2, "both rows of the tab state their value in {panel:?}");
    for state in &states {
        for button in &buttons {
            assert!(
                !state.intersects(*button),
                "the {} tab draws the state {state:?} under the button {button:?}",
                tab.label()
            );
        }
    }
}

/// The first character of the tab row and of the heading is in the panel that
/// the person sees, and no canvas covers it.
fn assert_first_characters_show(
    shapes: &[egui::epaint::ClippedShape],
    panel: egui::Rect,
    canvas: egui::Rect,
    label: &str,
    _tab: super::SettingsTab,
) {
    let tab_label = text_start(shapes, "General").unwrap_or_else(|| panic!("the {label} tab draws the tab row"));
    assert!(
        panel.contains(tab_label) && !canvas.contains(tab_label),
        "the {label} tab draws the tab row at {tab_label:?}, the panel is {panel:?} and the canvas {canvas:?}"
    );
    let heading = panel_texts(shapes, panel)
        .into_iter()
        .filter(|(_, start)| start.y > tab_label.y)
        .min_by(|left, right| left.1.x.total_cmp(&right.1.x))
        .unwrap_or_else(|| panic!("the {label} tab draws its heading"));
    assert!(
        panel.contains(heading.1) && !canvas.contains(heading.1),
        "the {label} tab draws the heading {:?} at {:?}, the panel is {panel:?} and the canvas {canvas:?}",
        heading.0,
        heading.1
    );
    for (text, start) in panel_texts(shapes, panel) {
        assert!(
            start.x >= canvas.max.x && start.x >= panel.min.x - 1.0,
            "the {label} tab draws {text:?} at {start:?}, the panel starts at {} and the canvas is {canvas:?}",
            panel.min.x
        );
    }
}

/// The rows that a person opens to replace a value and to confirm a delete fit
/// the panel as well.
fn assert_open_rows_fit_the_panel(
    ctx: &egui::Context,
    app: &mut crate::app::HorizonApp,
    viewport: [f32; 2],
    first: egui::Rect,
) {
    let opened = horizon_core::named_secrets::SecretName::parse("HZ_DEMO_TOKEN").expect("a name");
    if let Some(editor) = app.settings.as_mut() {
        editor.active_tab = super::SettingsTab::Secrets;
        editor.secret_inputs = super::secrets::SecretInputs::with_open_rows(&opened);
    }
    let shapes = settings_frame(ctx, app, viewport, super::SettingsTab::Secrets);
    let panel = panel_background_rect(&shapes, reserved_panel_rect(app));
    assert!(
        same_rect(panel, first),
        "the open rows ask for {panel:?} where the panel is {first:?}"
    );
    for label in ["Store", "Cancel", "Delete"] {
        let rects = text_rects(&shapes, label);
        assert!(!rects.is_empty(), "the open rows draw {label}");
        for rect in rects {
            assert!(
                rect.min.x >= panel.min.x - 1.0 && rect.max.x <= panel.max.x + 1.0,
                "the open rows draw {label} at {rect:?} beside the panel {panel:?}"
            );
        }
    }
}

fn same_rect(left: egui::Rect, right: egui::Rect) -> bool {
    (left.min.x - right.min.x).abs() <= 1.0
        && (left.max.x - right.max.x).abs() <= 1.0
        && (left.min.y - right.min.y).abs() <= 1.0
        && (left.max.y - right.max.y).abs() <= 1.0
}

/// The widest panel background over three frames, after the panel settles.
fn widest_panel_width(ctx: &egui::Context, app: &mut crate::app::HorizonApp, viewport: [f32; 2]) -> f32 {
    let mut widths = Vec::new();
    for _ in 0..3 {
        let output = crate::app::test_support::run_app_frame_with_input(
            ctx,
            app,
            crate::app::test_support::raw_input(viewport, None),
        );
        widths.push(panel_background_rect(&output.shapes, reserved_panel_rect(app)).width());
    }
    widths.into_iter().fold(0.0_f32, f32::max)
}

#[test]
fn the_speech_tab_says_whether_the_key_is_here_and_shows_the_text_of_its_take() {
    use horizon_core::TranscriberKind;
    use horizon_core::named_secrets::{NamedSecretState, NamedSecretWorkbench};
    use horizon_core::speech::SpeechPlatform;

    let mut config = Config::default();
    config.speech.transcriber.kind = TranscriberKind::Service;
    // The recorder line of the person, because the default of the platform is a
    // value and not a `cfg!`: a Mac has no recorder of the system, so its default
    // line is empty and the tab then says that speech input is off. This task is
    // about the take, so it writes a line that both systems accept.
    config.speech.recorder = "rec --rate 16000 {file}".to_owned();
    let workbench = NamedSecretWorkbench::new(None, &[]);
    let answer: Result<String, String> = Ok("hei".to_owned());
    let view = super::speech::SpeechView {
        answer: Some(&answer),
        running: false,
        kept: None,
        secret: NamedSecretState::Present,
    };
    let (changed, texts) = super::frame_of(|ui| super::speech::render(ui, &mut config, &workbench, &view).changed);

    assert!(!changed, "drawing the tab changes no value");
    // The recorder row names the default of the platform of this build: the line
    // of `pw-record` on Linux, and the statement that a Mac has none instead of an
    // empty value. Both sentences are values, so this task reads the one of the
    // system that runs it and the tab states the same on both.
    let platform = SpeechPlatform::current();
    assert!(
        texts.iter().any(|text| text == &platform.default_recorder_sentence()),
        "the recorder row names the default of {}: {texts:?}",
        platform.name()
    );
    assert!(
        platform.default_recorder_sentence().contains(platform.name()),
        "the sentence names the platform that it is about: {}",
        platform.default_recorder_sentence()
    );
    assert!(
        texts_hold(&texts, "SPEECH_API_KEY: stored on this computer."),
        "the tab says that this computer holds the key: {texts:?}"
    );
    assert!(
        texts_hold(&texts, "hei"),
        "the tab shows the text of its take: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.contains("goes to the service of the endpoint")),
        "the tab says where a take goes: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.contains("Shift+R sends the last kept take again")),
        "the tab names the shortcut that sends a kept take again: {texts:?}"
    );
}

#[test]
fn the_speech_tab_keeps_the_language_empty_and_says_what_an_empty_one_means() {
    use horizon_core::TranscriberKind;
    use horizon_core::named_secrets::{NamedSecretState, NamedSecretWorkbench};

    let mut config = Config::default();
    config.speech.transcriber.kind = TranscriberKind::Service;
    let workbench = NamedSecretWorkbench::new(None, &[]);
    let view = super::speech::SpeechView {
        answer: None,
        running: false,
        kept: None,
        secret: NamedSecretState::NoValue,
    };
    let (_changed, texts) = super::frame_of(|ui| super::speech::render(ui, &mut config, &workbench, &view).changed);

    assert!(
        config.speech.transcriber.language.is_empty(),
        "the settings leave the language of a take to the service"
    );
    assert!(
        texts.iter().any(|text| text == super::speech::LANGUAGE_EXAMPLES),
        "the empty field shows the examples of a language hint: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text.contains(super::speech::EMPTY_LANGUAGE)),
        "the tab says what an empty language means: {texts:?}"
    );
}

#[test]
fn the_speech_tab_says_that_speech_input_is_off_while_no_transcriber_is_set() {
    let mut config = Config::default();
    let workbench = horizon_core::named_secrets::NamedSecretWorkbench::new(None, &[]);
    let view = super::speech::SpeechView {
        answer: None,
        running: false,
        kept: None,
        secret: horizon_core::named_secrets::NamedSecretState::NoValue,
    };
    let (_changed, texts) = super::frame_of(|ui| super::speech::render(ui, &mut config, &workbench, &view).changed);
    assert!(
        texts.iter().any(|text| text.contains("Speech input is off.")),
        "the tab says that speech input is off: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text.contains("standard output is the text")),
        "the tab shows the command line of its transcriber: {texts:?}"
    );
}

/// The panel of a window of 1480 points draws every tab with the longest values
/// that its fields take: a name of 64 characters, a recorder line of 200
/// characters, the 253-character name of a server, an open invitation, three
/// computers of the server, an import line and one line of a refused value in
/// every card that has one.
///
/// A word that its own card cuts is scrolled under the edge of the card, so the
/// person reads only the part inside the panel. The checks here judge that part:
/// no word of the panel lies under a control of the panel, no word lies on
/// another, and the panel keeps one rectangle whatever its values are.
#[test]
fn the_panel_holds_the_longest_values_of_every_tab() {
    // The width of a window of 1480 points gives the panel 444 points. The window
    // is taller here, so every card of every tab is drawn and not scrolled out of
    // the view.
    let viewport = [1480.0_f32, 2600.0];
    let (temp, ctx, mut app, _names) = app_with_stored_secrets();
    seed_server_tab(&mut app, temp.path());
    seed_long_values(&mut app);

    let mut panels = Vec::new();
    let mut drawn: Vec<(&str, Vec<String>)> = Vec::new();
    for tab in super::SettingsTab::ALL {
        let shapes = settings_frame(&ctx, &mut app, viewport, *tab);
        let panel = panel_background_rect(&shapes, reserved_panel_rect(&app));
        panels.push((tab.label(), panel));
        let words = assert_tab_keeps_its_words_clear(&shapes, panel, tab.label());
        drawn.push((tab.label(), words.into_iter().map(|(text, _)| text).collect()));
    }
    let texts_of = |tab: &str| -> &[String] {
        &drawn
            .iter()
            .find(|(label, _)| *label == tab)
            .unwrap_or_else(|| panic!("the panel drew {tab}"))
            .1
    };

    // The long values of the test reached the tabs that show them, so the checks
    // above judged the widest line of each tab and not an empty panel.
    assert!(
        texts_of("Server")
            .iter()
            .any(|text| text.starts_with("aaaa") && text.chars().count() == 253),
        "the Server tab draws the 253-character name of the server: {:?}",
        texts_of("Server")
    );
    assert!(
        texts_of("Remote").iter().any(|text| text.starts_with("Imported ")),
        "the Remote tab draws the line of its last import: {:?}",
        texts_of("Remote")
    );
    assert!(
        texts_of("Remote")
            .iter()
            .any(|text| text.contains("TS_ABCDEFGHIJKLMNOP_CLIENT_ID")
                && text.contains("TS_ABCDEFGHIJKLMNOP_CLIENT_SECRET")
                && text.contains("TS_ABCDEFGHIJKLMNOP_TAG")),
        "the Remote tab draws the three names of the network: {:?}",
        texts_of("Remote")
    );
    assert!(
        texts_of("Speech")
            .iter()
            .any(|text| text.contains("--rate 16000 {file}") && text.chars().count() > 190),
        "the Speech tab draws the recorder line of 200 characters: {:?}",
        texts_of("Speech")
    );
    assert!(
        texts_of("Presets")
            .iter()
            .any(|text| text.starts_with("a-preset-with-a-long-name-")),
        "the Presets tab draws the card of its long preset: {:?}",
        texts_of("Presets")
    );
    assert!(
        texts_of("Keys")
            .iter()
            .any(|text| text == "Ctrl+Alt+Shift+Super+Hyper+Meta+K"),
        "the Keys tab draws the binding that it refuses: {:?}",
        texts_of("Keys")
    );
    for (tab, line) in [
        ("General", "remote_hosts.vnc_port must be nonzero"),
        ("General", "browser_host.ssh_target must be one SSH name"),
        ("Browser", "browser.quality must be between 1 and 100"),
        ("Server", "server.command must be 1 to 200 characters"),
        ("Speech", "speech.hotkey needs a modifier or a function key"),
    ] {
        assert!(
            texts_of(tab).iter().any(|text| text.contains(line)),
            "the {tab} tab draws the line of its refused value {line:?}: {:?}",
            texts_of(tab)
        );
    }

    let (first_tab, first) = panels.first().copied().expect("eight tabs");
    for (tab, panel) in &panels {
        assert!(
            same_rect(*panel, first),
            "the panel is {first:?} on {first_tab} and {panel:?} on {tab}"
        );
    }
    let given = super::settings_panel_default_width(viewport[0]);
    assert!(
        first.width() <= given + 2.0,
        "the longest values ask for {first:?} and the window gives the panel {given}"
    );
}

/// The visible part of every word of the panel, with the words that a control
/// covers and the words that lie on each other of one tab.
fn assert_tab_keeps_its_words_clear(
    shapes: &[egui::epaint::ClippedShape],
    panel: egui::Rect,
    label: &str,
) -> Vec<(String, egui::Rect)> {
    let mut words: Vec<(String, egui::Rect)> = Vec::new();
    let mut controls: Vec<(String, egui::Rect)> = Vec::new();
    for_each_painted(shapes, &mut |shape, painted, clip| {
        let egui::Shape::Text(drawn) = shape else {
            return;
        };
        // The panel is one part of the window, so only the text that a clip of the
        // panel draws is a word of the tab.
        if !panel.expand(1.0).contains_rect(clip) {
            return;
        }
        // The part that the person reads is the part that the clip of the card
        // leaves. A field that scrolls its value keeps the rest out of the panel.
        let visible = painted.intersect(clip);
        if !visible.is_positive() {
            return;
        }
        let text = drawn.galley.text();
        if super::panel_test_support::TAB_BUTTON_LABELS.contains(&text) {
            controls.push((text.to_owned(), visible));
        } else {
            words.push((text.to_owned(), visible));
        }
    });

    for (word, rect) in &words {
        assert!(
            panel.expand(1.0).contains_rect(*rect),
            "the {label} tab draws the word {word:?} at {rect:?} beside the panel {panel:?}"
        );
        for (control, control_rect) in &controls {
            assert!(
                !rect.intersects(*control_rect),
                "the {label} tab draws the word {word:?} at {rect:?} under the control {control:?} at {control_rect:?}"
            );
        }
    }
    for (index, (left, left_rect)) in words.iter().enumerate() {
        for (right, right_rect) in &words[index + 1..] {
            assert!(
                !left_rect.intersects(*right_rect),
                "the {label} tab draws the words {left:?} and {right:?} on each other at {left_rect:?} and {right_rect:?}"
            );
        }
    }
    // A control keeps its own place as well: one button over another is one
    // button that a person cannot press.
    for (control, rect) in &controls {
        assert!(
            panel.expand(1.0).contains_rect(*rect),
            "the {label} tab draws the control {control:?} at {rect:?} beside the panel {panel:?}"
        );
    }
    for (index, (left, left_rect)) in controls.iter().enumerate() {
        for (right, right_rect) in &controls[index + 1..] {
            assert!(
                !left_rect.intersects(*right_rect),
                "the {label} tab draws the controls {left:?} and {right:?} on each other at {left_rect:?} and {right_rect:?}"
            );
        }
    }
    words
}

/// The longest values that the fields of the tabs take, and one line of a refused
/// value in every card that shows one.
fn seed_long_values(app: &mut crate::app::HorizonApp) {
    let Some(editor) = app.settings.as_mut() else {
        panic!("the settings window is open");
    };
    // The line of a recorder of this computer: 200 characters with the file that
    // the take replaces.
    editor.config.speech.recorder = format!("{} --rate 16000 {{file}}", "recorder-of-the-north-".repeat(8));
    // The name of the server: 253 characters, which is the longest name of a
    // computer that a name of the domain system takes.
    let server_name = ["a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(61)].join(".");
    assert_eq!(server_name.chars().count(), 253);
    editor.config.server.ssh_target = Some(server_name);
    // The outcome of the last import of a portable profile, with the path that the
    // person typed.
    let import_path = format!("/home/person/{}/remote-profile.json", "a-folder-".repeat(16));
    let notice = format!("Imported {import_path}: 3 providers and 4 targets.");
    editor.portable_profile = super::remote_browsers::PortableProfilePanel::with_notice(import_path, (notice, false));
    // One preset of the Presets tab, with a name and a command line of the length
    // that its rule allows.
    editor.config.presets.push(horizon_core::PresetConfig {
        name: "a-preset-with-a-long-name-".repeat(4),
        alias: Some("a-long-alias-of-a-preset".to_owned()),
        kind: horizon_core::PanelKind::Command,
        command: Some(format!("{} --flag", "a-command-line-of-a-preset ".repeat(8))),
        args: Vec::new(),
        resume: horizon_core::PanelResume::Fresh,
        ssh_connection: None,
    });
    // One binding of the Keys tab that no key of this computer names, so the row
    // shows its indicator and the card shows the line of its rule.
    editor.config.shortcuts.command_palette = "Ctrl+Alt+Shift+Super+Hyper+Meta+K".to_owned();
    // The three names of a network of an older Horizon, at the 16 characters that
    // `<NET>` takes, with one of them on this computer already: the longest line
    // that the card of the keyring draws.
    let net = "ABCDEFGHIJKLMNOP".to_string();
    let line = horizon_core::remote_browser_credential::TailnetLine::of(net, [true, false, false])
        .expect("the three names of the recipe");
    app.keyring_import
        .adopt_offer(horizon_core::remote_browser_credential::TailnetOffer::of(vec![line]));
    // One refused value in every card that shows the line of its rule.
    editor.config.remote_hosts.vnc_port = 0;
    editor.config.browser.quality = 0;
    editor.config.browser_host.ssh_target = Some("a target with a space".to_owned());
    editor.config.speech.hotkey = "Q".to_owned();
    editor.config.server.command = Some("a command with a \"quote\"".to_owned());
    editor.config.secrets.names.push("a name with a space in it".to_owned());
}

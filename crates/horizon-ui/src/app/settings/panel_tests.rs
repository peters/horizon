//! The panel of the Settings window: its width, its tab row, the rows of the
//! Server tab, and the rectangle that the canvas leaves beside it.

use horizon_core::{Config, RuntimeState, StartupDecision};

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

/// No word of the panel lies under a button of the panel: the row of a computer
/// gives its text the width that its buttons leave, and a longer line wraps under
/// itself instead of under a control.
fn assert_no_word_lies_under_a_button(shapes: &[egui::epaint::ClippedShape]) {
    let mut words = Vec::new();
    let mut buttons = Vec::new();
    for_each_painted(shapes, &mut |shape, painted, _clip| {
        if let egui::Shape::Text(drawn) = shape {
            let text = drawn.galley.text();
            if BUTTON_LABELS.contains(&text) {
                buttons.push(painted);
            } else {
                words.push((text.to_owned(), painted));
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

/// The labels of the controls of the settings panel, so a test knows which text
/// of the panel is a word and which one is a control.
const BUTTON_LABELS: &[&str] = &[
    "Add a computer",
    "Copy link",
    "Delete",
    "Join",
    "Keep",
    "Read the list",
    "Remove",
    "Replace value",
    "Store",
    "Cancel",
    "Save",
    "Revert",
    "Close",
    "Add",
    "Reset Defaults",
];

/// One link whose child ends at once, one open invitation and three computers of
/// the server, so the Server tab draws its plate, its link and its list.
fn seed_server_tab(app: &mut crate::app::HorizonApp, directory: &std::path::Path) {
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
    // allows, and one removal that waits for its confirmation.
    editor.server.devices = vec![
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
fn app_with_stored_secrets() -> (tempfile::TempDir, egui::Context, crate::app::HorizonApp, [String; 2]) {
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
fn settings_frame(
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
    crate::app::test_support::run_app_frame_with_input(ctx, app, crate::app::test_support::raw_input(viewport, None))
}

/// Checks one tab against the panel and the canvas, and answers the rectangle of
/// the panel that it paints.
fn assert_tab_fits_the_panel(
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

/// Runs `visit` over every painted primitive, with the rectangle that it paints
/// and the rectangle that it is clipped to.
fn for_each_painted(
    shapes: &[egui::epaint::ClippedShape],
    visit: &mut impl FnMut(&egui::Shape, egui::Rect, egui::Rect),
) {
    for clipped in shapes {
        visit_shape(&clipped.shape, clipped.clip_rect, visit);
    }
}

fn visit_shape(shape: &egui::Shape, clip: egui::Rect, visit: &mut impl FnMut(&egui::Shape, egui::Rect, egui::Rect)) {
    if let egui::Shape::Vec(shapes) = shape {
        for shape in shapes {
            visit_shape(shape, clip, visit);
        }
        return;
    }
    // A shape without vertices, such as an empty mesh, has no rectangle.
    let painted = shape.visual_bounding_rect();
    if [painted.min.x, painted.min.y, painted.max.x, painted.max.y]
        .iter()
        .all(|value| value.is_finite())
    {
        visit(shape, painted, clip);
    }
}

/// The rectangle that the panel reserved in the last frame, which is where its
/// background begins.
fn reserved_panel_rect(app: &crate::app::HorizonApp) -> egui::Rect {
    app.settings
        .as_ref()
        .and_then(|editor| editor.panel_rect)
        .expect("the open panel reserved its rectangle")
}

/// The rectangle that the Settings panel paints: the tallest shape that covers
/// the whole width that the panel reserved, which is its background. The
/// rectangle of that shape is what the panel asked for, and the clip of a panel
/// that overflows is still the panel, so only the rectangle tells a panel that
/// fits from one that does not.
fn panel_background_rect(shapes: &[egui::epaint::ClippedShape], reserved: egui::Rect) -> egui::Rect {
    let mut panel: Option<egui::Rect> = None;
    for_each_painted(shapes, &mut |_, painted, _| {
        if painted.min.x > reserved.min.x + 2.0 || painted.max.x < reserved.max.x - 2.0 {
            return;
        }
        if panel.is_none_or(|current| painted.height() > current.height()) {
            panel = Some(painted);
        }
    });
    panel.unwrap_or(egui::Rect::NOTHING)
}

/// The clip rectangle of the canvas: the widest clip of the frame that starts at
/// the window corner and stops before the window edge. The canvas is the only
/// surface with such a rectangle; the toolbar and the overlays cover the whole
/// window, and the sidebar and the panel start beside a corner.
fn canvas_background_clip(shapes: &[egui::epaint::ClippedShape], viewport: [f32; 2]) -> egui::Rect {
    let mut widest: Option<egui::Rect> = None;
    for clipped in shapes {
        let clip = clipped.clip_rect;
        if clip.min.x.abs() > 0.5 || clip.min.y.abs() > 0.5 || clip.width() >= viewport[0] - 0.5 {
            continue;
        }
        if widest.is_none_or(|current| clip.width() > current.width()) {
            widest = Some(clip);
        }
    }
    widest.unwrap_or(egui::Rect::NOTHING)
}

/// The rectangles of one drawn text.
fn text_rects(shapes: &[egui::epaint::ClippedShape], text: &str) -> Vec<egui::Rect> {
    let mut rects = Vec::new();
    for_each_painted(shapes, &mut |shape, painted, _| {
        if let egui::Shape::Text(drawn) = shape
            && drawn.galley.text() == text
        {
            rects.push(painted);
        }
    });
    rects
}

/// The texts that the panel paints, with the place where each one starts.
fn panel_texts(shapes: &[egui::epaint::ClippedShape], panel: egui::Rect) -> Vec<(&str, egui::Pos2)> {
    let mut texts = Vec::new();
    for clipped in shapes {
        if !panel.expand(1.0).contains_rect(clipped.clip_rect) {
            continue;
        }
        collect_panel_texts(&clipped.shape, &mut texts);
    }
    texts
}

fn collect_panel_texts<'a>(shape: &'a egui::Shape, texts: &mut Vec<(&'a str, egui::Pos2)>) {
    match shape {
        egui::Shape::Text(drawn) => texts.push((drawn.galley.text(), drawn.pos)),
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_panel_texts(shape, texts);
            }
        }
        _ => {}
    }
}

/// Where the first character of one drawn text begins.
fn text_start(shapes: &[egui::epaint::ClippedShape], text: &str) -> Option<egui::Pos2> {
    for clipped in shapes {
        if let Some(start) = text_start_in(&clipped.shape, text) {
            return Some(start);
        }
    }
    None
}

fn text_start_in(shape: &egui::Shape, text: &str) -> Option<egui::Pos2> {
    match shape {
        egui::Shape::Text(drawn) if drawn.galley.text() == text => Some(drawn.pos),
        egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_start_in(shape, text)),
        _ => None,
    }
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

    let mut config = Config::default();
    config.speech.transcriber.kind = TranscriberKind::Service;
    let workbench = NamedSecretWorkbench::new(None, &[]);
    let answer: Result<String, String> = Ok("hei".to_owned());
    let view = super::speech::SpeechView {
        answer: Some(&answer),
        running: false,
        secret: NamedSecretState::Present,
    };
    let (changed, texts) = super::frame_of(|ui| super::speech::render(ui, &mut config, &workbench, &view).changed);

    assert!(!changed, "drawing the tab changes no value");
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
}

#[test]
fn the_speech_tab_says_that_speech_input_is_off_while_no_transcriber_is_set() {
    let mut config = Config::default();
    let workbench = horizon_core::named_secrets::NamedSecretWorkbench::new(None, &[]);
    let view = super::speech::SpeechView {
        answer: None,
        running: false,
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

/// The footer of the panel: its status line and its four buttons. The bar is the
/// widest shape of the last band of the panel, and every text of that band is
/// either the label of a button or the status line.
fn footer_texts(shapes: &[egui::epaint::ClippedShape], panel: egui::Rect) -> Vec<(String, egui::Rect)> {
    let band = footer_band(shapes, panel);
    let mut texts = Vec::new();
    for_each_painted(shapes, &mut |shape, painted, clip| {
        // A text that the clip of the tab cuts is scrolled under the bar, so the
        // person does not read it in the footer.
        if let egui::Shape::Text(drawn) = shape
            && clip.contains_rect(painted)
            && painted.center().y >= band.min.y + 4.0
            && painted.center().y <= band.max.y - 4.0
        {
            texts.push((drawn.galley.text().to_owned(), painted));
        }
    });
    texts
}

/// The band that the footer bar covers: the widest shape that ends at the bottom
/// of the panel content and is as tall as the bar.
fn footer_band(shapes: &[egui::epaint::ClippedShape], panel: egui::Rect) -> egui::Rect {
    let mut band = egui::Rect::NOTHING;
    for_each_painted(shapes, &mut |_shape, painted, _clip| {
        let at_the_bottom = painted.max.y <= panel.max.y + 1.0 && panel.max.y - painted.max.y <= 20.0;
        if at_the_bottom && painted.height() > 40.0 && painted.height() < 56.0 && painted.width() > band.width() {
            band = painted;
        }
    });
    assert!(!band.is_negative(), "the panel {panel:?} draws its footer bar");
    band
}

/// One state of the save that the footer shows.
#[derive(Clone, Copy)]
enum FooterState {
    None,
    LivePreview,
    Saving,
    Saved,
    Error,
}

impl FooterState {
    /// The state of the editor, as the frame that applies the save sets it.
    fn status(self, error: &str) -> super::SettingsStatus {
        match self {
            Self::None => super::SettingsStatus::None,
            Self::LivePreview => super::SettingsStatus::LivePreview,
            Self::Saving => super::SettingsStatus::Saving,
            Self::Saved => super::SettingsStatus::Saved,
            Self::Error => super::SettingsStatus::Error(error.to_owned()),
        }
    }

    /// The text of the footer: the state, or the line of an error that is longer
    /// than the place of the status line.
    fn text(self) -> Option<&'static str> {
        match self {
            Self::LivePreview => Some("Live preview"),
            Self::Saving => Some("Saving"),
            Self::Saved => Some("Saved"),
            Self::None | Self::Error => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::None => "no state",
            Self::LivePreview => "live preview",
            Self::Saving => "saving",
            Self::Saved => "saved",
            Self::Error => "an error",
        }
    }
}

/// The footer keeps its text and its buttons in the panel, whatever the tab and
/// whatever the state of the save says.
///
/// The status line takes the width that the buttons leave, so no state can put a
/// button over it: a footer that draws "Live preview" under the Close button
/// reads "Live prev" and hides the rest.
#[test]
fn the_footer_row_fits_the_panel_in_every_state() {
    let viewport = [1480.0_f32, 900.0];
    let (_temp, ctx, mut app, _names) = app_with_stored_secrets();
    let error = "The endpoint of the transcriber must be an address that starts with https, and this \
                 line of an error is longer than the place of the status line";
    let cases = [
        (FooterState::None, false),
        (FooterState::LivePreview, true),
        (FooterState::Saving, true),
        (FooterState::Saved, false),
        (FooterState::Error, true),
    ];

    for (case, dirty) in cases {
        let state = case.name();
        for tab in super::SettingsTab::ALL {
            if let Some(editor) = app.settings.as_mut() {
                editor.status = case.status(error);
                editor.dirty = dirty;
            }
            let shapes = settings_frame(&ctx, &mut app, viewport, *tab);
            let panel = panel_background_rect(&shapes, reserved_panel_rect(&app));
            let band = footer_band(&shapes, panel);
            let texts = footer_texts(&shapes, panel);
            let buttons: Vec<egui::Rect> = texts
                .iter()
                .filter(|(text, _)| BUTTON_LABELS.contains(&text.as_str()))
                .map(|(_, rect)| *rect)
                .collect();
            let expected_buttons = if dirty { 4 } else { 3 };
            assert_eq!(
                buttons.len(),
                expected_buttons,
                "the {} tab draws {expected_buttons} buttons of its footer in {state}: {texts:?}",
                tab.label()
            );

            // The buttons are in the panel, and in the row that the bar gives them.
            for button in &buttons {
                assert!(
                    button.min.x >= panel.min.x + 1.0 && button.max.x <= panel.max.x - 1.0,
                    "the {state} footer draws a button at {button:?} beside the panel {panel:?}"
                );
                assert!(
                    band.expand(1.0).contains_rect(*button),
                    "the {state} footer draws a button at {button:?} and its bar is {band:?}"
                );
            }

            // The status line is the one text of the band that is not a button, it
            // keeps the place that the buttons leave, and it shows the state.
            let lines: Vec<&(String, egui::Rect)> = texts
                .iter()
                .filter(|(text, _)| !BUTTON_LABELS.contains(&text.as_str()))
                .collect();
            let line_text = match case {
                FooterState::None => {
                    assert!(
                        lines.is_empty(),
                        "the {state} footer of the {} tab draws no line: {lines:?}",
                        tab.label()
                    );
                    continue;
                }
                FooterState::Error => error,
                other => other.text().unwrap_or_default(),
            };
            assert_eq!(
                lines.len(),
                1,
                "the {state} footer of the {} tab draws one line: {texts:?}",
                tab.label()
            );
            let (drawn, line) = lines[0];
            for button in &buttons {
                assert!(
                    !line.intersects(*button) && line.max.x + 6.0 <= button.min.x,
                    "the {state} footer draws {drawn:?} at {line:?} under the button {button:?}"
                );
            }
            assert!(
                band.expand(1.0).contains_rect(*line),
                "the {state} footer draws {drawn:?} at {line:?} and its bar is {band:?}"
            );
            assert_eq!(
                drawn,
                line_text,
                "the {} tab says the state of the save in its footer",
                tab.label()
            );
            if matches!(case, FooterState::Error) {
                // A line of an error can be longer than the place of the status
                // line. It is then cut inside that place, and the person reads the
                // whole line when the pointer rests on it.
                let whole = status_line_width(&ctx, error);
                assert!(
                    line.width() + 1.0 < whole,
                    "a line of {whole} points is cut in a place of {} points: {line:?}",
                    line.width()
                );
            }
        }
    }
}

/// The width of `text` in the font and the size of the status line of the footer.
fn status_line_width(ctx: &egui::Context, text: &str) -> f32 {
    ctx.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(12.0), egui::Color32::WHITE)
            .size()
            .x
    })
}

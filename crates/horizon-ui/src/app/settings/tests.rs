use horizon_core::{Config, RuntimeState, StartupDecision};

use super::SettingsStatus;
use crate::app::test_support::test_app_with_startup;

/// The texts that one headless frame of `render` draws, and whether that frame
/// changed a value of the draft.
pub(crate) fn frame_of(mut render: impl FnMut(&mut egui::Ui) -> bool) -> (bool, Vec<String>) {
    use crate::test_egui::DiscardTextures;

    let ctx = egui::Context::default();
    let mut changed = None;
    let output = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    changed = Some(render(ui));
                });
            });
        })
        .discard_textures();
    let mut texts = Vec::new();
    for clipped in &output.shapes {
        collect_texts(&clipped.shape, &mut texts);
    }
    (changed.expect("the frame rendered the content"), texts)
}

/// Whether `text` is one of the drawn texts.
pub(crate) fn texts_hold(texts: &[String], text: &str) -> bool {
    texts.iter().any(|drawn| drawn == text)
}

/// Whether one of the drawn texts is `text`, which is what a number and its
/// suffix draw as two texts.
pub(crate) fn at(texts: &[String], text: &str) -> bool {
    texts_hold(texts, text)
}

fn collect_texts(shape: &egui::Shape, texts: &mut Vec<String>) {
    match shape {
        egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_texts(shape, texts);
            }
        }
        _ => {}
    }
}

#[test]
fn the_general_tab_draws_a_field_for_every_remote_host_setting() {
    let mut config = Config::default();
    let (changed, texts) = frame_of(|ui| super::general::render(ui, &mut config));

    assert!(!changed, "a frame that a person did not edit changes nothing");
    for expected in ["Remote hosts", "Workspace", "VNC port", "Remote Sessions"] {
        assert!(
            texts_hold(&texts, expected),
            "the section draws a field for {expected}: {texts:?}"
        );
    }

    // A host with a port of its own is one row, with the button that removes it.
    config.remote_hosts.vnc_ports.insert("smoke-node".to_owned(), 5997);
    let (changed, texts) = frame_of(|ui| super::general::render(ui, &mut config));
    assert!(!changed);
    assert!(texts_hold(&texts, "smoke-node"), "{texts:?}");
    assert!(texts_hold(&texts, "Remove"), "{texts:?}");
}

#[test]
fn the_tab_row_fits_one_line_at_the_default_panel_width() {
    use crate::test_egui::DiscardTextures;

    let labels: Vec<&str> = super::SettingsTab::ALL.iter().map(|tab| tab.label()).collect();
    let ctx = egui::Context::default();
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

    // The default window of this build, so the panel has its default width.
    let panel = super::settings_panel_default_width(1600.0);
    let content = panel - 2.0 * f32::from(super::SETTINGS_PANEL_MARGIN_X);
    assert!(
        needed <= content,
        "the {labels:?} tabs need {needed} and the panel gives {content}"
    );
}

#[test]
fn every_tab_draws_on_one_line_at_the_default_panel_width() {
    use crate::test_egui::DiscardTextures;

    let panel = super::settings_panel_default_width(1600.0);
    let content = panel - 2.0 * f32::from(super::SETTINGS_PANEL_MARGIN_X);
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
    };
    let ctx = egui::Context::default();
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
    for tab in ["General", "Shortcuts", "Presets", "Browser", "Remote", "Secrets"] {
        assert!(texts_hold(&texts, tab), "the row draws {tab}: {texts:?}");
    }
}

#[test]
fn a_port_row_gives_the_host_box_a_useful_width_and_a_hint() {
    let mut config = Config::default();
    let (_changed, texts) = frame_of(|ui| super::general::render(ui, &mut config));
    assert!(
        texts_hold(&texts, "Host label or SSH host name"),
        "the empty host box says what belongs in it: {texts:?}"
    );

    // The box keeps the room that its row needs: it is never narrower than a host
    // name and never wide enough to strand the port far from the host.
    assert!((super::general::host_field_width(0.0) - 140.0).abs() < f32::EPSILON);
    assert!((super::general::host_field_width(432.0) - 260.0).abs() < f32::EPSILON);
    assert!((super::general::host_field_width(300.0) - 146.0).abs() < f32::EPSILON);
}

#[test]
fn the_remote_hosts_section_shows_the_rule_of_a_refused_entry() {
    let mut config = Config::default();
    config.remote_hosts.default_workspace = "  ".to_string();
    config.remote_hosts.vnc_ports.insert("  ".to_string(), 5900);
    let (changed, texts) = frame_of(|ui| super::general::render(ui, &mut config));

    assert!(!changed);
    assert!(
        texts
            .iter()
            .any(|text| text.ends_with("remote_hosts.default_workspace cannot be empty")),
        "the rule of the workspace is under its field: {texts:?}"
    );
}

fn ephemeral() -> StartupDecision {
    StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    }
}

#[test]
fn a_save_shows_saving_and_then_saved() {
    let (_temp, _ctx, mut app) = test_app_with_startup(ephemeral());
    app.toggle_settings();
    if let Some(editor) = app.settings.as_mut() {
        editor.config.appearance.theme = horizon_core::AppearanceTheme::Light;
        editor.dirty = true;
    }

    app.save_settings();
    assert!(
        matches!(app.settings.as_ref().expect("open").status, SettingsStatus::Saving),
        "the status line says that the write runs"
    );

    app.wait_for_settings_io();
    let editor = app.settings.as_ref().expect("open");
    assert!(
        matches!(editor.status, SettingsStatus::Saved),
        "the status line says that the write finished"
    );
    assert_eq!(
        app.template_config.appearance.theme,
        horizon_core::AppearanceTheme::Light
    );
}

/// A settings file that lies beside the store is not a settings source: the
/// editor shows the settings of the store, so a later save cannot put the
/// edited file into the store.
#[test]
fn a_settings_file_beside_the_store_never_reaches_the_editor() {
    let (_temp, _ctx, mut app) = test_app_with_startup(ephemeral());
    Config::save(&app.config_path, &app.template_config).expect("write the store");
    std::fs::write(
        &app.config_path,
        "{\n  \"version\": 11,\n  \"appearance\": {\n    \"theme\": \"light\"\n  }\n}\n",
    )
    .expect("write the edited file");

    app.toggle_settings();

    let editor = app.settings.as_ref().expect("open");
    assert_eq!(
        editor.config.appearance.theme,
        horizon_core::AppearanceTheme::Auto,
        "the editor holds the settings of the store, not the file beside it"
    );
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
    let (_temp, ctx, mut app, _names) = app_with_stored_secrets();

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

/// The two machine entries have a field in General, so a render must keep the
/// values that the fields edit: a section that rebuilt its entry would drop a
/// host or a server without a word.
#[test]
fn the_machine_fields_keep_their_entries_while_they_render() {
    let mut config = Config::default();
    config.browser_host.ssh_target = Some("browser-vm".into());
    config.browser_host.profile = Some("mail".into());
    config.server.ssh_target = Some("server-vm".into());
    config.server.command = Some("/opt/horizon --server-link".into());

    let _ = crate::test_egui::DiscardTextures::discard_textures(render_general_tab(&mut config));

    assert_eq!(config.browser_host.ssh_target.as_deref(), Some("browser-vm"));
    assert_eq!(config.browser_host.profile.as_deref(), Some("mail"));
    assert_eq!(config.server.ssh_target.as_deref(), Some("server-vm"));
    assert_eq!(config.server.command.as_deref(), Some("/opt/horizon --server-link"));
}

/// One headless pass of the General tab, with the textures handled so a
/// dropped frame does not report an unapplied delta.
fn render_general_tab(config: &mut Config) -> egui::FullOutput {
    let ctx = egui::Context::default();
    ctx.run_ui(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            super::general::render(ui, config);
        });
    })
}

/// The Secrets tab draws one row per name, the state of the value on this computer,
/// and the fields that add one. A value that a person typed is never drawn.
#[test]
fn the_secrets_tab_draws_the_names_the_hidden_field_and_the_missing_value() {
    use std::sync::{Arc, Mutex};

    use horizon_core::named_secrets::{NamedSecretVault, SecretName};
    use horizon_core::remote_browser_credential::SharedCredentialStore;
    use horizon_core::secret_store::SecretStore;

    let values = Arc::new(Mutex::new(SecretStore::in_memory().expect("a store in memory")));
    let stored = SecretName::parse("HETZNER_TOKEN").expect("a name");
    values
        .lock()
        .expect("the store")
        .write("named/HETZNER_TOKEN", b"a-value-of-this-computer")
        .expect("the value");
    let vault = Arc::new(NamedSecretVault::new(Box::new(SharedCredentialStore::new(Arc::clone(
        &values,
    )))));
    let mut config = Config::default();
    config.secrets.names = vec!["HETZNER_TOKEN".to_string(), "CLOUD_KEY".to_string()];
    let names = config.secrets.secret_names();
    vault.set_names(config.secrets.names.clone());
    let mut bench = horizon_core::named_secrets::NamedSecretWorkbench::new(Some(vault), &names);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while bench.is_busy() && std::time::Instant::now() < deadline {
        bench.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let mut inputs = super::secrets::SecretInputs::with_typed("NEW_KEY", "TYPED-VALUE-6f1c");
    let (_changed, texts) = frame_of(|ui| super::secrets::render(ui, &mut config, &mut bench, &mut inputs));

    for expected in [
        "HETZNER_TOKEN",
        "CLOUD_KEY",
        "Secrets for agents",
        "Replace value",
        "Delete",
        "Add",
        "2 of 64 names",
    ] {
        assert!(texts_hold(&texts, expected), "the tab draws {expected}: {texts:?}");
    }
    assert!(
        texts.iter().any(|text| text.contains("no value on this computer")),
        "a name whose value this computer does not hold says so: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text.contains("is not here: type it once more")),
        "the tab says that a value of an older Horizon must be typed once more: {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text.contains("TYPED-VALUE-6f1c")),
        "a typed value is never drawn: {texts:?}"
    );
    assert_eq!(
        inputs.typed(),
        ("NEW_KEY".to_string(), "TYPED-VALUE-6f1c".to_string()),
        "a frame that drew the fields keeps the draft of the person"
    );
    assert!(matches!(
        bench.state(&stored),
        horizon_core::named_secrets::NamedSecretState::Present
    ));
}

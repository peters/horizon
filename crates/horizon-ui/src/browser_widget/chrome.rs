//! Browser panel chrome strip: navigation buttons, URL bar, ownership and
//! passkey chip, and the handoff banner.

use egui::{
    Role, Stroke, TextEdit, TextWrapMode, Ui, WidgetInfo,
    text::{CCursor, CCursorRange},
    vec2,
};
use horizon_core::browser::{
    BackendAvailability, BackendKind, BrowserCommand, BrowserPanelState, BrowserVideoOperation, BrowserVideoState,
    normalize_navigation_target,
};

use crate::browser_widget::BrowserUiState;
use crate::theme;

const CHROME_HEIGHT: f32 = 30.0;
/// The URL bar keeps at least this much width even when an owner chip is
/// present on a narrow panel.
const URL_MIN_WIDTH: f32 = 120.0;

/// The text of the chrome at the size it has on the screen of the canvas zoom.
///
/// A stock label or button lays its text out at the size of zoom 1, and the layer
/// transform of the canvas then scales a finished picture of every letter, so the
/// engine name, the pill and the notice lines blur at any zoom but 100 percent.
/// The galley keeps the size and the line of zoom 1 and its letters are made for
/// the screen size, so the row sits where it did and stays sharp. `width` is the
/// room the text may take; a text that does not fit ends with an ellipsis.
fn chrome_text(ui: &Ui, text: &str, size: f32, color: egui::Color32, width: Option<f32>) -> egui::WidgetText {
    crate::zoom_text::widget_text(ui.painter(), text, egui::FontId::proportional(size), color, width)
}
/// The size of the address of the page, in screen points.
const URL_FONT_SIZE: f32 = 11.5;
const VIDEO_QUEUE_REJECTED: &str = "browser command queue is full; video command was not queued";

/// Draw the top strip. Returns whether the URL bar has focus and whether
/// any chrome widget was clicked this frame (for panel-focus requests).
pub fn show(
    ui: &mut Ui,
    panel_id: horizon_core::PanelId,
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    interactive: bool,
) -> (bool, bool) {
    if let Some(identity) = browser.remote_identity_display() {
        let disclosure = browser
            .active_backend_capabilities()
            .map(|active| active.automation_disclosure.as_str());
        remote_identity_header(ui, identity, disclosure);
    }
    let chrome_row = ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if browser.is_remote() {
            ui.spacing_mut().button_padding = vec2(4.0, 4.0);
        }
        ui.set_min_height(CHROME_HEIGHT);
        // Reserve the chip's width up front so the URL bar's expanding
        // width cannot starve it (a trailing `right_to_left` scope gets
        // zero remaining width and the chip renders invisibly).
        let chip = ownership_chip_label(browser);
        let natural_chip_width = chip.as_ref().map_or(0.0, |(label, _)| {
            ui.painter()
                .layout_no_wrap(label.clone(), egui::FontId::proportional(10.0), egui::Color32::WHITE)
                .size()
                .x
        });
        let mut clicked = false;
        clicked |= nav_button(ui, "←", "Back", browser, BrowserCommand::Back, interactive);
        clicked |= nav_button(ui, "→", "Forward", browser, BrowserCommand::Forward, interactive);
        clicked |= nav_button(ui, "⟳", "Reload", browser, BrowserCommand::Reload, interactive);
        clicked |= video_controls(ui, browser, interactive);
        clicked |=
            state
                .screenshots
                .copy_button(ui, interactive && crate::screenshot::browser_available(browser), || {
                    crate::screenshot::browser_image(browser)
                });
        clicked |= super::orientation::controls(ui, browser, interactive);
        clicked |= backend_picker(ui, panel_id, browser, interactive);
        profile_label(ui, browser);
        // Measure after the nav buttons so the cap fits the real remainder.
        // The owner name is an unrestricted external string: cap the chip to
        // what the row can spare while keeping the URL bar a usable minimum
        // width, and let the chip truncate (full name on hover). Without a
        // chip the bar keeps its old full-width behavior.
        let chip_width = if chip.is_some() {
            natural_chip_width.min((ui.available_width() - URL_MIN_WIDTH - 8.0).max(0.0))
        } else {
            0.0
        };
        let url_max_width = if chip.is_some() {
            (ui.available_width() - chip_width - 8.0).max(URL_MIN_WIDTH)
        } else {
            f32::INFINITY
        };
        let (url_focused, url_clicked) = url_bar(ui, panel_id, browser, state, interactive, url_max_width);
        clicked |= url_clicked;
        if let Some((label, color)) = &chip {
            ui.add_sized(
                egui::vec2(chip_width, CHROME_HEIGHT),
                egui::Label::new(chrome_text(ui, label, 10.0, *color, Some(chip_width)))
                    .wrap_mode(TextWrapMode::Truncate),
            )
            .on_hover_text(label.clone());
        }
        (url_focused, clicked)
    });
    let (url_focused, mut clicked) = chrome_row.inner;
    super::orientation::status(ui, browser);

    if let Some(error) = &browser.navigation_error {
        let text = chrome_text(ui, error, 10.5, theme::PALETTE_RED(), Some(ui.available_width()));
        ui.add(egui::Label::new(text));
    }

    if browser.page_resumed {
        let text = chrome_text(
            ui,
            "The link to the browser was lost and this is the same page again: the text, the scroll place and the session \
             are still there.",
            10.5,
            theme::PALETTE_YELLOW(),
            Some(ui.available_width()),
        );
        ui.add(egui::Label::new(text));
    }

    if matches!(browser.status, horizon_core::browser::BrowserStatus::Ready)
        && let Some(message) = upload_limit_line(UploadLimit {
            remote_device: browser.remote_target().is_some(),
            on_browser_host: browser.runs_on_browser_host(),
            chooser_supported: browser.frame_slot.file_chooser().supported(),
            page_asked: browser.frame_slot.file_chooser().status().pending(),
        })
    {
        let text = chrome_text(ui, message, 10.5, theme::PALETTE_YELLOW(), Some(ui.available_width()));
        ui.add(egui::Label::new(text));
    }
    let reason = browser.handoff_reason.clone();
    if let Some(reason) = reason {
        clicked |= handoff_banner(ui, browser, &reason, interactive);
    }
    clicked |= crate::browser_widget::teach::show(ui, browser, interactive);
    (url_focused, clicked)
}

/// What the upload line of a browser panel needs of that panel and its page.
#[derive(Clone, Copy, Default)]
struct UploadLimit {
    remote_device: bool,
    on_browser_host: bool,
    chooser_supported: bool,
    page_asked: bool,
}

/// The words of the yellow line under the chrome for a panel that cannot take a
/// file of this computer, or `None` while the panel says nothing.
///
/// A page of a browser host never takes a file of this computer, and the note
/// line of the panel says that once, when the panel connects. The yellow line
/// waits until a page of that host asks for a file, because that is the moment a
/// person needs the words.
fn upload_limit_line(limit: UploadLimit) -> Option<&'static str> {
    let UploadLimit {
        remote_device,
        on_browser_host,
        chooser_supported,
        page_asked,
    } = limit;
    if chooser_supported {
        return None;
    }
    if remote_device {
        return Some("A remote device of a provider cannot take files from this computer.");
    }
    if on_browser_host {
        return page_asked.then_some(
            "A page of that browser asked for a file. A browser host cannot take a file of this computer: open \
             the file dialog of the page on that machine.",
        );
    }
    Some(
        "Manual upload selection is unavailable. Use local Chromium, or update local Firefox to a build with \
         file-dialog support.",
    )
}

fn remote_identity_header(
    ui: &mut Ui,
    identity: &horizon_core::browser::RemoteIdentityDisplay,
    disclosure: Option<&str>,
) {
    // A dedicated row keeps the hover target usable at the minimum panel width.
    // Disclosure is read here because the cached identity text is built before
    // the session publishes its status. The tooltip closure runs only while open.
    ui.add_sized(
        vec2(ui.available_width(), CHROME_HEIGHT),
        egui::Label::new(identity.label())
            .wrap_mode(TextWrapMode::Truncate)
            .show_tooltip_when_elided(false),
    )
    .on_hover_ui(|ui| {
        ui.set_max_width(crate::text::stable_tooltip_max_width(ui));
        ui.add(egui::Label::new(identity.tooltip()).wrap());
        if let Some(status) = disclosure {
            ui.label(status);
        }
    });
}

/// The picker acts only for an interactive local panel that is not teaching.
fn picker_enabled(browser: &BrowserPanelState, interactive: bool) -> bool {
    interactive && browser.teach().is_none() && !browser.is_remote() && !browser.is_attached()
}

/// The SSH target of the browser host of this panel. The target is the name that
/// `ssh` resolves, never an endpoint of the host.
fn remote_host_label(ui: &mut Ui, target: &str) {
    ui.add(egui::Label::new(chrome_text(
        ui,
        target,
        10.0,
        theme::FG_SOFT(),
        Some(ui.available_width()),
    )))
    .on_hover_text("This page belongs to a browser host on this SSH target");
}

/// The profile of a browser panel, beside its engine: `Shared: <name>` is one
/// browser of the sign-ins of that profile, and `Private` is a panel that is
/// signed in nowhere.
///
/// The label is a pill of the size of the engine name beside it. The shared name
/// is neutral, because it is the ordinary case; `Private` carries the accent,
/// because a person must notice that this panel reads none of the sign-ins.
fn profile_label(ui: &mut Ui, browser: &horizon_core::browser::BrowserPanelState) {
    const SHARED_HINT: &str = "Every panel of this profile is one page of one browser: they read the same sign-ins, and a sign-out in one \
         signs out all of them. Use Private panel for a browser that is not signed in.";
    const PRIVATE_HINT: &str = "This panel is signed in nowhere: it reads none of the shared sign-ins, and what it keeps is gone when \
         the panel closes.";
    let profile = match (browser.visible_profile_name(), browser.is_private_page()) {
        (Some(name), _) => ProfilePill {
            text: format!("Shared: {name}"),
            hint: SHARED_HINT,
            private: false,
        },
        (None, true) => ProfilePill {
            text: "Private".to_string(),
            hint: PRIVATE_HINT,
            private: true,
        },
        (None, false) => return,
    };
    profile.show(ui);
}

/// One pill of the profile of a browser panel.
struct ProfilePill {
    text: String,
    hint: &'static str,
    private: bool,
}

impl ProfilePill {
    fn show(self, ui: &mut Ui) {
        // The picker of the engine draws its name in the button style of this
        // window, so the pill takes that size and the two read as one row.
        let size = ui
            .style()
            .text_styles
            .get(&egui::TextStyle::Button)
            .map_or(12.0, |font| font.size);
        let color = if self.private {
            theme::ACCENT()
        } else {
            theme::FG_SOFT()
        };
        let text = chrome_text(ui, &self.text, size, color, Some(ui.available_width()));
        egui::Frame::new()
            .fill(theme::PANEL_BG_ALT())
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.add(egui::Label::new(text).wrap_mode(TextWrapMode::Truncate))
                    .on_hover_text(self.hint);
            });
    }
}

fn backend_picker(
    ui: &mut Ui,
    panel_id: horizon_core::PanelId,
    browser: &mut BrowserPanelState,
    interactive: bool,
) -> bool {
    // Remote panels show the family and disclosure on the identity header.
    // A picker on that row would be a second fixed control.
    if browser.is_remote() {
        return false;
    }
    // The engine of a page of a browser host is fixed by the profile of that
    // host, so the picker shows the host instead of offering another engine.
    if let Some(target) = browser.attached_host_target() {
        remote_host_label(ui, target);
        return false;
    }
    let previous = browser.backend();
    let mut selected = previous;
    let selected_text = browser.active_backend_capabilities().map_or_else(
        || previous.display_name().to_string(),
        |active| {
            if active.backend == BackendKind::SafariWebDriver && active.bidi {
                "Safari + BiDi".to_string()
            } else {
                previous.display_name().to_string()
            }
        },
    );
    let enabled = picker_enabled(browser, interactive);
    let picker = ui.add_enabled_ui(enabled, |ui| {
        egui::ComboBox::from_id_salt(("browser-backend", panel_id))
            .selected_text(selected_text)
            .width(72.0)
            .show_ui(ui, |ui| {
                for backend in [
                    BackendKind::ChromiumCdp,
                    BackendKind::FirefoxBidi,
                    BackendKind::SafariWebDriver,
                ] {
                    match backend.availability() {
                        BackendAvailability::Available => {
                            ui.selectable_value(&mut selected, backend, backend.display_name());
                        }
                        BackendAvailability::UnsupportedPlatform(reason) => {
                            ui.add_enabled_ui(false, |ui| {
                                ui.selectable_value(&mut selected, backend, backend.display_name())
                            })
                            .inner
                            .on_hover_text(reason);
                        }
                    }
                }
            })
    });
    if let Some(status) = browser
        .active_backend_capabilities()
        .map(|active| active.automation_disclosure.as_str())
    {
        // The button owns hover input; the enclosing enabled scope does not.
        picker.inner.response.on_hover_text(status);
    }
    if selected == previous {
        return false;
    }
    browser.switch_backend(selected);
    true
}

fn video_controls(ui: &mut Ui, browser: &mut BrowserPanelState, interactive: bool) -> bool {
    let capture = browser.video_capture();
    let state = capture.as_ref().map(|capture| capture.state);
    if matches!(state, Some(BrowserVideoState::Recording | BrowserVideoState::Paused)) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
    let mut clicked = false;
    match state {
        Some(BrowserVideoState::Recording) => {
            let elapsed = capture.as_ref().map_or(0, |capture| capture.elapsed_millis);
            let text = chrome_text(ui, &format_elapsed(elapsed), 10.5, theme::PALETTE_RED(), None);
            ui.add(egui::Label::new(text));
            clicked |= video_button(
                ui,
                "⏸",
                "Pause recording",
                browser,
                BrowserVideoOperation::Pause,
                interactive,
            );
            clicked |= video_stop_button(ui, browser, interactive);
        }
        Some(BrowserVideoState::Paused) => {
            let elapsed = capture.as_ref().map_or(0, |capture| capture.elapsed_millis);
            let paused = chrome_text(
                ui,
                &format!("{} paused", format_elapsed(elapsed)),
                10.5,
                theme::PALETTE_YELLOW(),
                None,
            );
            ui.add(egui::Label::new(paused));
            clicked |= video_button(
                ui,
                "▶",
                "Resume recording",
                browser,
                BrowserVideoOperation::Resume,
                interactive,
            );
            clicked |= video_stop_button(ui, browser, interactive);
        }
        Some(BrowserVideoState::Stopped) | None => {
            let hover = if let Some(error) = browser.video_error.as_deref() {
                format!("Record browser session to WebM ({error})")
            } else if capture.as_ref().is_some_and(|capture| capture.encoder_failed) {
                format!(
                    "Record browser session to WebM (encoder failed: {})",
                    capture.as_ref().map_or("-", |capture| capture.path.as_str())
                )
            } else {
                capture.as_ref().map_or_else(
                    || "Record browser session to WebM".to_string(),
                    |capture| format!("Record browser session to WebM (last: {})", capture.path),
                )
            };
            clicked |= video_start_button(ui, &hover, browser, interactive);
        }
    }
    clicked
}

fn video_start_button(ui: &mut Ui, hover: &str, browser: &mut BrowserPanelState, interactive: bool) -> bool {
    let can_record = interactive && browser.can_start_recording();
    let response = crate::icon_button::icon_button(ui, can_record, "Record", |painter, rect, _color| {
        crate::icon_button::paint_record(painter, rect, can_record);
    })
    .on_hover_text_at_pointer(hover);
    if response.clicked() {
        browser.video_error = None;
        if !browser.try_send(BrowserCommand::Video {
            operation: BrowserVideoOperation::Start,
            options: None,
        }) {
            browser.video_error = Some(VIDEO_QUEUE_REJECTED.to_string());
        }
        return true;
    }
    false
}

fn video_stop_button(ui: &mut Ui, browser: &mut BrowserPanelState, interactive: bool) -> bool {
    let label = browser.video_error.as_deref().map_or_else(
        || "Stop recording".to_string(),
        |error| format!("Stop recording ({error})"),
    );
    let response = crate::icon_button::icon_button(ui, interactive, "Stop recording", crate::icon_button::paint_stop)
        .on_hover_text_at_pointer(label);
    if response.clicked() {
        if !browser.try_send(BrowserCommand::Video {
            operation: BrowserVideoOperation::Stop,
            options: None,
        }) {
            browser.video_error = Some(VIDEO_QUEUE_REJECTED.to_string());
        }
        return true;
    }
    false
}

fn video_button(
    ui: &mut Ui,
    glyph: &str,
    label: &str,
    browser: &mut BrowserPanelState,
    operation: BrowserVideoOperation,
    interactive: bool,
) -> bool {
    let response = ui.add_enabled(
        interactive,
        egui::Button::new(chrome_text(ui, glyph, 13.0, ui.visuals().text_color(), None))
            .min_size(vec2(22.0, 22.0))
            .fill(theme::PANEL_BG_ALT())
            .corner_radius(6)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE())),
    );
    let enabled = response.enabled();
    response.widget_info(|| nav_widget_info(label, enabled));
    let response = response.on_hover_text_at_pointer(label);
    if response.clicked() {
        if !browser.try_send(BrowserCommand::Video {
            operation,
            options: None,
        }) {
            browser.video_error = Some(VIDEO_QUEUE_REJECTED.to_string());
        }
        return true;
    }
    false
}

fn format_elapsed(millis: u64) -> String {
    let total_secs = millis / 1_000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{mins:02}:{secs:02}")
}

fn nav_button(
    ui: &mut Ui,
    glyph: &str,
    label: &str,
    browser: &BrowserPanelState,
    command: BrowserCommand,
    interactive: bool,
) -> bool {
    let response = ui.add_enabled(
        interactive,
        egui::Button::new(chrome_text(ui, glyph, 13.0, ui.visuals().text_color(), None))
            .min_size(vec2(22.0, 22.0))
            .fill(theme::PANEL_BG_ALT())
            .corner_radius(6)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE())),
    );
    let enabled = response.enabled();
    response.widget_info(|| nav_widget_info(label, enabled));
    let response = response.on_hover_text_at_pointer(format!("{label} (browser)"));
    if response.clicked() {
        browser.send(command);
        return true;
    }
    false
}

fn nav_widget_info(label: &str, enabled: bool) -> WidgetInfo {
    WidgetInfo::labeled(Role::Button, enabled, label)
}

fn url_bar(
    ui: &mut Ui,
    panel_id: horizon_core::PanelId,
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    interactive: bool,
    max_width: f32,
) -> (bool, bool) {
    let id = ui.make_persistent_id(("browser-url-bar", panel_id));
    let was_focused = ui.memory(|memory| memory.focused() == Some(id));
    // Keep the buffer in sync with the live URL while unfocused.
    sync_url_buffer(&mut state.url_buffer, browser.display_url(), was_focused);
    // The panel of a browser is a panel of the canvas, so its address is under
    // the canvas zoom: it is laid out at the size of the screen, like the text
    // of every other panel.
    let font = egui::FontId::monospace(URL_FONT_SIZE);
    let mut layouter = |ui: &Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
        let job = egui::text::LayoutJob::simple(buffer.as_str().to_owned(), font.clone(), theme::FG(), wrap_width);
        crate::zoom_text::layout_job(ui.painter(), job)
    };
    let response = ui.add_enabled(
        interactive,
        TextEdit::singleline(&mut state.url_buffer)
            .id(id)
            .hint_text("Enter address")
            .desired_width(max_width)
            .min_size(vec2(if browser.is_remote() { URL_MIN_WIDTH } else { 0.0 }, 0.0))
            .font(egui::FontId::monospace(URL_FONT_SIZE))
            .layouter(&mut layouter),
    );
    // Focusing selects the stable canonical URL without rewriting it. Page
    // interaction can move focus away from the field, and that transition
    // must not make scrolling look like navigation.
    if response.gained_focus() && !response.changed() {
        let mut edit_state = TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        edit_state.cursor.set_char_range(Some(CCursorRange::two(
            CCursor::default(),
            CCursor::new(state.url_buffer.chars().count()),
        )));
        edit_state.store(ui.ctx(), id);
        ui.ctx().request_repaint();
    }
    // egui's singleline TextEdit consumes Enter and drops its own focus
    // (the default `return_key` shortcut), so "focus was lost on the same
    // frame as an Enter press" is the submit signal.
    let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if submitted {
        state.url_submit_enter_pending = !ui.input(|i| i.key_released(egui::Key::Enter));
        let url = state.url_buffer.trim().to_string();
        let target = normalize_navigation_target(&url);
        if !url.is_empty() && browser.display_url() != target {
            // A driver-less submission is retained as the relaunch target and
            // surfaced via navigation_error instead of being dropped. The
            // guard compares the displayed target (pending first), not the
            // committed URL: resubmitting the persisted URL while a newer
            // pending target is queued must replace it, not be skipped.
            browser.submit_navigation(&target);
        }
    }
    (response.has_focus() || submitted, response.clicked())
}

fn sync_url_buffer(url_buffer: &mut String, canonical_url: &str, was_focused: bool) -> bool {
    if was_focused || url_buffer == canonical_url {
        return false;
    }

    url_buffer.clear();
    url_buffer.push_str(canonical_url);
    true
}

/// The chip's label and color, or `None` when neither an agent owner nor a
/// pending handoff should be surfaced.
fn ownership_chip_label(browser: &BrowserPanelState) -> Option<(String, egui::Color32)> {
    chip_label(
        browser.handoff_reason.is_some(),
        browser.frame_slot.passkey_pending(),
        browser.owner.as_deref(),
    )
}

/// A handoff wait comes first; a passkey request waiting for the external
/// authenticator command comes before the owning agent's name.
fn chip_label(handoff: bool, passkey: bool, owner: Option<&str>) -> Option<(String, egui::Color32)> {
    if handoff {
        Some((String::from("waiting"), theme::PALETTE_YELLOW()))
    } else if passkey {
        Some((String::from("passkey request waiting"), theme::PALETTE_YELLOW()))
    } else {
        owner.map(|owner| (format!("agent: {owner}"), theme::PALETTE_GREEN()))
    }
}

fn handoff_banner(ui: &mut Ui, browser: &mut BrowserPanelState, reason: &str, interactive: bool) -> bool {
    let mut clicked = false;
    ui.scope(|ui| {
        ui.visuals_mut().override_text_color = Some(theme::PALETTE_YELLOW());
        // Single-line horizontal strip. Do not use a `right_to_left` row here:
        // under a plain top-down parent the truncated label's wrap width
        // collapses in egui 0.36, growing the banner to the full body height
        // and hiding the page frame behind it.
        ui.horizontal(|ui| {
            let button_label = if browser.handoff_resolution_pending {
                "Handing back…"
            } else {
                "Done — hand back to agent"
            };
            let button_width = ui
                .painter()
                .layout_no_wrap(
                    button_label.to_owned(),
                    egui::FontId::proportional(11.0),
                    egui::Color32::WHITE,
                )
                .size()
                .x
                + (ui.spacing().button_padding.x * 2.0);
            let label_width = (ui.available_width() - button_width - ui.spacing().item_spacing.x).max(0.0);
            let paused_line = chrome_text(
                ui,
                &format!("🖐 Agent paused: {reason}"),
                12.0,
                ui.visuals().text_color(),
                Some(label_width),
            );
            ui.add_sized([label_width, 0.0], egui::Label::new(paused_line))
                .on_hover_text(reason);
            let hand_back = chrome_text(ui, button_label, 11.0, ui.visuals().text_color(), None);
            clicked = ui
                .add_enabled(
                    interactive && !browser.handoff_resolution_pending,
                    egui::Button::new(hand_back)
                        .fill(theme::blend(theme::PANEL_BG_ALT(), theme::PALETTE_YELLOW(), 0.2))
                        .corner_radius(6),
                )
                .clicked();
        });
        if let Some(error) = &browser.handoff_error {
            let text = chrome_text(
                ui,
                &format!("Could not hand back: {error}"),
                10.5,
                theme::PALETTE_RED(),
                Some(ui.available_width()),
            );
            ui.add(egui::Label::new(text));
        }
    });
    if clicked {
        browser.hand_back();
    }
    clicked
}

#[cfg(test)]
mod tests {
    use horizon_core::browser::{BackendKind, BrowserPanelState, BrowserStatus};

    use super::{UploadLimit, backend_picker, nav_widget_info, picker_enabled, sync_url_buffer, upload_limit_line};
    use crate::test_egui::DiscardTextures;

    #[test]
    fn the_upload_line_of_a_browser_host_waits_for_a_page() {
        let on_host = UploadLimit {
            on_browser_host: true,
            ..UploadLimit::default()
        };
        assert_eq!(
            upload_limit_line(on_host),
            None,
            "a panel of a browser host says the limit in its note line and no yellow line"
        );
        let asked = upload_limit_line(UploadLimit {
            page_asked: true,
            ..on_host
        })
        .expect("a page that asked");
        assert!(
            asked.contains("browser host") && asked.contains("cannot take a file"),
            "{asked}"
        );
        assert_eq!(
            upload_limit_line(UploadLimit {
                chooser_supported: true,
                ..UploadLimit {
                    page_asked: true,
                    ..on_host
                }
            }),
            None,
            "a chooser that serves the files shows no line"
        );
        assert!(
            upload_limit_line(UploadLimit::default())
                .expect("local")
                .contains("local Chromium"),
            "a local Firefox of an old build keeps its own words"
        );
        assert!(
            upload_limit_line(UploadLimit {
                remote_device: true,
                ..UploadLimit::default()
            })
            .expect("provider device")
            .contains("remote device"),
            "a device of a provider keeps its own words"
        );
    }

    #[test]
    fn a_remote_panel_shows_session_state_without_changing_its_backend() {
        let mut remote = BrowserPanelState::restored_remote(
            "remote-picker",
            &horizon_core::browser::BrowserConfig {
                backend: BackendKind::SafariWebDriver,
                ..horizon_core::browser::BrowserConfig::default()
            },
            "ios_phone".to_string(),
            None,
        );
        assert!(!picker_enabled(&remote, true), "a remote target fixes the browser");
        assert_eq!(
            remote.backend(),
            BackendKind::SafariWebDriver,
            "the backend family is unchanged"
        );

        // The remote identity label leaves the backend unchanged.
        let ctx = egui::Context::default();
        let mut changed = None;
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                changed = Some(backend_picker(ui, horizon_core::PanelId(1), &mut remote, true));
            })
            .discard_textures();
        assert_eq!(changed, Some(false));
        assert_eq!(remote.backend(), BackendKind::SafariWebDriver);

        let local = BrowserPanelState::inert();
        assert!(picker_enabled(&local, true));
        assert!(!picker_enabled(&local, false), "a non-interactive view never picks");
    }

    #[test]
    fn disclosure_status_stays_visible_on_a_narrow_panel() {
        for width in [720.0, 280.0] {
            assert!(
                hover_shows_status(local_disclosure_panel(), "common_signals_minimized", width),
                "local picker hid common_signals_minimized at width {width}"
            );
            assert!(
                hover_shows_status(remote_disclosure_panel(), "unsupported_by_backend", width),
                "remote identity hid unsupported_by_backend at width {width}"
            );
        }
    }

    fn local_disclosure_panel() -> BrowserPanelState {
        let local = BrowserPanelState::inert();
        local
            .frame_slot
            .publish_backend_capabilities_for_tests(disclosure_capabilities(
                BackendKind::FirefoxBidi,
                true,
                horizon_browser::AutomationDisclosureStatus::CommonSignalsMinimized,
            ));
        local
    }

    fn remote_disclosure_panel() -> BrowserPanelState {
        let remote = BrowserPanelState::inert_remote("ios_phone", "browserstack");
        remote
            .frame_slot
            .publish_backend_capabilities_for_tests(disclosure_capabilities(
                BackendKind::ChromiumCdp,
                false,
                horizon_browser::AutomationDisclosureStatus::UnsupportedByBackend,
            ));
        remote
    }

    fn disclosure_capabilities(
        backend: BackendKind,
        bidi: bool,
        automation_disclosure: horizon_browser::AutomationDisclosureStatus,
    ) -> horizon_browser::ActiveBackendCapabilities {
        horizon_browser::ActiveBackendCapabilities {
            backend,
            capabilities: horizon_browser::BackendCapabilities::remote_session(),
            bidi,
            automation_disclosure,
        }
    }

    fn hover_shows_status(mut browser: BrowserPanelState, status: &str, width: f32) -> bool {
        let height = 220.0;
        for y in [4.0, 14.0, 24.0, 34.0, 44.0, 54.0] {
            let mut x = 4.0;
            while x < width {
                if status_inside(&mut browser, status, width, height, egui::pos2(x, y)) {
                    return true;
                }
                x += 12.0;
            }
        }
        false
    }

    fn status_inside(
        browser: &mut BrowserPanelState,
        status: &str,
        width: f32,
        height: f32,
        pointer: egui::Pos2,
    ) -> bool {
        let ctx = egui::Context::default();
        let mut pass = |time: f64, moved: bool| {
            ctx.run_ui(hover_input_size(pointer, time, width, height, moved), |ui| {
                let mut state = crate::browser_widget::BrowserUiState::default();
                super::show(ui, horizon_core::PanelId(2), browser, &mut state, true);
            })
            .discard_textures()
        };
        let _ = pass(0.0, true);
        let _ = pass(0.1, false);
        let _ = pass(1.0, false);
        shape_has_status(&pass(1.1, false), status, width, height)
    }

    fn shape_has_status(output: &egui::FullOutput, status: &str, width: f32, height: f32) -> bool {
        fn walk(shape: &egui::Shape, status: &str, width: f32, height: f32) -> bool {
            match shape {
                egui::Shape::Text(text) if text.galley.job.text.contains(status) => {
                    let bounds = text.visual_bounding_rect();
                    bounds.width() > 0.0
                        && bounds.left() >= -1.0
                        && bounds.right() <= width + 1.0
                        && bounds.top() >= -1.0
                        && bounds.bottom() <= height + 1.0
                }
                egui::Shape::Vec(shapes) => shapes.iter().any(|shape| walk(shape, status, width, height)),
                _ => false,
            }
        }
        output
            .shapes
            .iter()
            .any(|shape| walk(&shape.shape, status, width, height))
    }

    #[test]
    fn remote_identity_hover_shows_the_disclosure_status() {
        let remote = BrowserPanelState::inert_remote("ios_phone", "browserstack");
        remote
            .frame_slot
            .publish_backend_capabilities_for_tests(horizon_browser::ActiveBackendCapabilities {
                backend: BackendKind::ChromiumCdp,
                capabilities: horizon_browser::BackendCapabilities::remote_session(),
                bidi: false,
                automation_disclosure: horizon_browser::AutomationDisclosureStatus::UnsupportedByBackend,
            });
        assert!(
            hover_shows_status(remote, "unsupported_by_backend", 720.0),
            "remote identity hover must show the disclosure status"
        );
    }

    fn hover_input_size(pointer: egui::Pos2, time: f64, width: f32, height: f32, moved: bool) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height))),
            time: Some(time),
            events: if moved {
                vec![egui::Event::PointerMoved(pointer)]
            } else {
                Vec::new()
            },
            ..egui::RawInput::default()
        }
    }

    #[test]
    fn long_remote_identity_is_single_line_truncated_with_room_for_the_url() {
        let mut remote = BrowserPanelState::inert_remote("target", "provider");
        remote.apply_remote_session_event_for_tests(horizon_browser::RemoteSessionEvent::DeviceIdentity {
            label: "target".into(),
            identity: horizon_browser::RemoteDeviceIdentity {
                browser_name: Some("safari".into()),
                model: Some("iPhone 16 Pro Max with a very long provider-reported device name".into()),
                os_name: Some("iOS".into()),
                os_version: Some("18.5".into()),
                ..horizon_browser::RemoteDeviceIdentity::default()
            },
        });
        remote.owner = Some("Test owner".into());
        for width in [320.0, 360.0, 500.0] {
            let ctx = egui::Context::default();
            let mut state = crate::browser_widget::BrowserUiState::default();
            let output = ctx
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 180.0))),
                        ..egui::RawInput::default()
                    },
                    |ui| {
                        super::show(ui, horizon_core::PanelId(1), &mut remote, &mut state, true);
                        assert!(ui.min_rect().right() <= width);
                    },
                )
                .discard_textures();
            let text = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text.starts_with("Remote browser") => Some(text),
                    _ => None,
                })
                .expect("identity text");
            assert_eq!(text.galley.rows.len(), 1);
            assert!(text.galley.elided, "long identity must elide at width {width}");
            assert!(text.galley.size().x > 200.0, "identity remains readable and hoverable");
            let bounds = text.visual_bounding_rect();
            assert!(
                bounds.left() >= 0.0 && bounds.right() <= width,
                "width={width}, bounds={bounds:?}"
            );
        }
    }

    #[test]
    fn address_bar_focus_transition_preserves_the_displayed_url() {
        let mut buffer = String::new();
        let original = "https://example.com/path?q=1#result";

        assert!(sync_url_buffer(&mut buffer, original, false));
        assert_eq!(buffer, original);
        assert!(!sync_url_buffer(&mut buffer, original, true));
        assert!(!sync_url_buffer(&mut buffer, original, false));
        assert_eq!(buffer, original);

        let navigated = "https://example.com/next";
        assert!(sync_url_buffer(&mut buffer, navigated, false));
        assert_eq!(buffer, navigated);
    }

    #[test]
    fn navigation_widget_info_names_glyph_only_controls() {
        for label in ["Back", "Forward", "Reload"] {
            let info = nav_widget_info(label, true);
            assert_eq!(info.role, egui::Role::Button);
            assert!(info.enabled);
            assert_eq!(info.label.as_deref(), Some(label));
        }

        assert!(!nav_widget_info("Back", false).enabled);
    }

    fn disabled_label(labels: &[(String, bool)], name: &str) -> bool {
        labels
            .iter()
            .find(|(label, _)| label == name)
            .map_or_else(|| panic!("missing {name} in {labels:?}"), |(_, disabled)| *disabled)
    }

    #[test]
    fn record_and_camera_icons_keep_accessibility_labels() {
        let idle = |interactive| {
            let mut browser = BrowserPanelState::inert();
            let mut state = crate::browser_widget::BrowserUiState::default();
            crate::test_egui::accesskit_labels(|ui| {
                super::show(ui, horizon_core::PanelId(1), &mut browser, &mut state, interactive);
            })
        };

        let stopped = idle(true);
        assert!(
            disabled_label(&stopped, "Record"),
            "a stopped browser cannot accept a recording command"
        );
        assert!(
            disabled_label(&stopped, "Copy screenshot"),
            "the camera stays disabled until a browser frame exists"
        );

        for status in [BrowserStatus::Ready, BrowserStatus::Starting] {
            let mut browser = BrowserPanelState::inert();
            let name = format!("{status:?}");
            browser.status = status;
            let mut state = crate::browser_widget::BrowserUiState::default();
            let labels = crate::test_egui::accesskit_labels(|ui| {
                super::show(ui, horizon_core::PanelId(1), &mut browser, &mut state, true);
            });
            assert!(
                disabled_label(&labels, "Record"),
                "{name} without a command channel cannot record"
            );
            assert!(disabled_label(&labels, "Copy screenshot"));
        }

        let inactive = idle(false);
        assert!(disabled_label(&inactive, "Record"));
        assert!(disabled_label(&inactive, "Copy screenshot"));

        let mut browser = BrowserPanelState::inert();
        let enabled_stop = crate::test_egui::accesskit_labels(|ui| {
            assert!(!super::video_stop_button(ui, &mut browser, true));
        });
        assert!(!disabled_label(&enabled_stop, "Stop recording"));
        let disabled_stop = crate::test_egui::accesskit_labels(|ui| {
            assert!(!super::video_stop_button(ui, &mut browser, false));
        });
        assert!(disabled_label(&disabled_stop, "Stop recording"));
    }

    /// The chrome beside the address field is made for the screen size of the
    /// canvas zoom: the engine name, the pill and a notice line stay sharp at a
    /// zoom of 67 and 150 percent, where a stock label would be a scaled picture
    /// of zoom 1.
    #[test]
    fn the_text_of_the_chrome_is_made_for_the_screen_size_of_the_zoom() {
        for display_scale in [1.0_f32, 2.0] {
            for zoom in [0.67_f32, 1.0, 1.5] {
                let (quads, worst_phase, worst_stretch) =
                    crate::zoom_text::measure_canvas_quads(display_scale, zoom, |ui, _| {
                        let notice = super::chrome_text(
                            ui,
                            "The link to the browser was lost and this is the same page again.",
                            10.5,
                            crate::theme::PALETTE_YELLOW(),
                            Some(460.0),
                        );
                        ui.add(egui::Label::new(notice));
                        let pill = super::chrome_text(ui, "Shared: work", 12.0, crate::theme::FG_SOFT(), None);
                        ui.add(egui::Label::new(pill));
                        let engine = super::chrome_text(ui, "firefox", 12.0, ui.visuals().text_color(), None);
                        ui.add(egui::Button::new(engine).min_size(egui::vec2(22.0, 22.0)));
                    });
                assert!(quads > 40, "the chrome draws its text: {quads}");
                assert!(
                    worst_phase <= 0.01,
                    "at zoom {zoom} and display scale {display_scale} a glyph corner sits {worst_phase} device pixels from a whole pixel"
                );
                assert!(
                    worst_stretch <= 0.01,
                    "at zoom {zoom} and display scale {display_scale} a glyph picture is stretched by {worst_stretch} device pixels"
                );
            }
        }
    }

    #[test]
    fn a_waiting_passkey_request_shows_before_the_owner_but_after_a_handoff() {
        let label = |handoff, passkey, owner| super::chip_label(handoff, passkey, owner).map(|(label, _)| label);
        assert_eq!(label(true, true, Some("codex")).as_deref(), Some("waiting"));
        assert_eq!(
            label(false, true, Some("codex")).as_deref(),
            Some("passkey request waiting")
        );
        assert_eq!(label(false, false, Some("codex")).as_deref(), Some("agent: codex"));
        assert_eq!(label(false, false, None), None);
    }
}

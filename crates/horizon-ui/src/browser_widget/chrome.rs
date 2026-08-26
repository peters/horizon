//! Browser panel chrome strip: navigation buttons, URL bar, ownership
//! chip, and the handoff banner.

use egui::{RichText, Stroke, TextEdit, TextWrapMode, Ui, WidgetInfo, WidgetType, vec2};
use horizon_core::browser::{BackendAvailability, BackendKind, BrowserCommand, BrowserPanelState};

use crate::browser_widget::BrowserUiState;
use crate::theme;

const CHROME_HEIGHT: f32 = 30.0;
/// The URL bar keeps at least this much width even when an owner chip is
/// present on a narrow panel.
const URL_MIN_WIDTH: f32 = 120.0;

/// Draw the top strip. Returns whether the URL bar has focus and whether
/// any chrome widget was clicked this frame (for panel-focus requests).
pub fn show(
    ui: &mut Ui,
    panel_id: horizon_core::PanelId,
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    interactive: bool,
) -> (bool, bool) {
    let chrome_row = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
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
        clicked |= backend_picker(ui, panel_id, browser, interactive);
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
                egui::Label::new(RichText::new(label.clone()).size(10.0).color(*color).strong())
                    .wrap_mode(TextWrapMode::Truncate),
            )
            .on_hover_text(label.clone());
        }
        (url_focused, clicked)
    });
    let (url_focused, mut clicked) = chrome_row.inner;

    if let Some(error) = &browser.navigation_error {
        ui.label(RichText::new(error).size(10.5).color(theme::PALETTE_RED()));
    }

    let reason = browser.handoff_reason.clone();
    if let Some(reason) = reason {
        clicked |= handoff_banner(ui, browser, &reason, interactive);
    }
    (url_focused, clicked)
}

fn backend_picker(
    ui: &mut Ui,
    panel_id: horizon_core::PanelId,
    browser: &mut BrowserPanelState,
    interactive: bool,
) -> bool {
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
    ui.add_enabled_ui(interactive, |ui| {
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
            });
    });
    if selected == previous {
        return false;
    }
    browser.switch_backend(selected);
    true
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
        egui::Button::new(RichText::new(glyph).size(13.0))
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
    WidgetInfo::labeled(WidgetType::Button, enabled, label)
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
    let display_url = browser.display_url();
    // Keep the buffer in sync with the live URL while unfocused.
    if ui.memory(|m| m.focused() != Some(id)) && state.url_buffer != display_url {
        state.url_buffer.clear();
        state.url_buffer.push_str(display_url);
    }
    let response = ui.add_enabled(
        interactive,
        TextEdit::singleline(&mut state.url_buffer)
            .id(id)
            .hint_text("https://…")
            .desired_width(max_width)
            .font(egui::FontId::monospace(11.5)),
    );
    // egui's singleline TextEdit consumes Enter and drops its own focus
    // (the default `return_key` shortcut), so "focus was lost on the same
    // frame as an Enter press" is the submit signal.
    let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if submitted {
        state.url_submit_enter_pending = !ui.input(|i| i.key_released(egui::Key::Enter));
        let url = state.url_buffer.trim().to_string();
        if !url.is_empty() && browser.display_url() != url {
            // A driver-less submission is retained as the relaunch target and
            // surfaced via navigation_error instead of being dropped. The
            // guard compares the displayed target (pending first), not the
            // committed URL: resubmitting the persisted URL while a newer
            // pending target is queued must replace it, not be skipped.
            browser.submit_navigation(&url);
        }
    }
    (response.has_focus() || submitted, response.clicked())
}

/// The chip's label and color, or `None` when neither an agent owner nor a
/// pending handoff should be surfaced.
fn ownership_chip_label(browser: &BrowserPanelState) -> Option<(String, egui::Color32)> {
    if browser.handoff_reason.is_some() {
        Some((String::from("waiting"), theme::PALETTE_YELLOW()))
    } else {
        browser
            .owner
            .as_ref()
            .map(|owner| (format!("agent: {owner}"), theme::PALETTE_GREEN()))
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
            ui.add_sized(
                [label_width, 0.0],
                egui::Label::new(RichText::new(format!("🖐 Agent paused: {reason}")).size(12.0))
                    .wrap_mode(TextWrapMode::Truncate),
            )
            .on_hover_text(reason);
            clicked = ui
                .add_enabled(
                    interactive && !browser.handoff_resolution_pending,
                    egui::Button::new(RichText::new(button_label).size(11.0))
                        .fill(theme::blend(theme::PANEL_BG_ALT(), theme::PALETTE_YELLOW(), 0.2))
                        .corner_radius(6),
                )
                .clicked();
        });
        if let Some(error) = &browser.handoff_error {
            ui.label(
                RichText::new(format!("Could not hand back: {error}"))
                    .size(10.5)
                    .color(theme::PALETTE_RED()),
            );
        }
    });
    if clicked {
        browser.hand_back();
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::nav_widget_info;

    #[test]
    fn navigation_widget_info_names_glyph_only_controls() {
        for label in ["Back", "Forward", "Reload"] {
            let info = nav_widget_info(label, true);
            assert_eq!(info.typ, egui::WidgetType::Button);
            assert!(info.enabled);
            assert_eq!(info.label.as_deref(), Some(label));
        }

        assert!(!nav_widget_info("Back", false).enabled);
    }
}

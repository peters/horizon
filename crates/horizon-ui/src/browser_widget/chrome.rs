//! Browser panel chrome strip: navigation buttons, URL bar, ownership
//! chip, and the handoff banner.

use egui::{RichText, Stroke, TextEdit, TextWrapMode, Ui, vec2};
use horizon_core::browser::{BrowserCommand, BrowserPanelState};

use crate::browser_widget::BrowserUiState;
use crate::theme;

const CHROME_HEIGHT: f32 = 30.0;

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
        let mut clicked = false;
        clicked |= nav_button(ui, "←", "Back", browser, BrowserCommand::Back, interactive);
        clicked |= nav_button(ui, "→", "Forward", browser, BrowserCommand::Forward, interactive);
        clicked |= nav_button(ui, "⟳", "Reload", browser, BrowserCommand::Reload, interactive);
        let (url_focused, url_clicked) = url_bar(ui, panel_id, browser, state, interactive);
        clicked |= url_clicked;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ownership_chip(ui, browser);
        });
        (url_focused, clicked)
    });
    let (url_focused, mut clicked) = chrome_row.inner;

    if let Some(error) = &browser.navigation_error {
        ui.label(RichText::new(error).size(10.5).color(theme::PALETTE_RED()));
    }

    let reason = browser.handoff_reason.clone();
    if let Some(reason) = reason {
        clicked |= handoff_banner(ui, browser, &reason);
    }
    (url_focused, clicked)
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
    let response = response.on_hover_text_at_pointer(format!("{label} (browser)"));
    if response.clicked() {
        browser.send(command);
        return true;
    }
    false
}

fn url_bar(
    ui: &mut Ui,
    panel_id: horizon_core::PanelId,
    browser: &BrowserPanelState,
    state: &mut BrowserUiState,
    interactive: bool,
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
            .desired_width(f32::INFINITY)
            .font(egui::FontId::monospace(11.5)),
    );
    // egui's singleline TextEdit consumes Enter and drops its own focus
    // (the default `return_key` shortcut), so "focus was lost on the same
    // frame as an Enter press" is the submit signal.
    let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if submitted {
        state.url_submit_enter_pending = !ui.input(|i| i.key_released(egui::Key::Enter));
        let url = state.url_buffer.trim().to_string();
        if !url.is_empty() && browser.committed_url_for_persistence() != Some(url.as_str()) {
            browser.send(BrowserCommand::Navigate(url));
        }
    }
    (response.has_focus() || submitted, response.clicked())
}

fn ownership_chip(ui: &mut Ui, browser: &BrowserPanelState) {
    let (label, color) = if browser.handoff_reason.is_some() {
        (String::from("waiting"), theme::PALETTE_YELLOW())
    } else if let Some(owner) = &browser.owner {
        (format!("agent: {owner}"), theme::PALETTE_GREEN())
    } else {
        return;
    };
    ui.label(RichText::new(label).size(10.0).color(color).strong());
}

fn handoff_banner(ui: &mut Ui, browser: &mut BrowserPanelState, reason: &str) -> bool {
    let mut clicked = false;
    ui.scope(|ui| {
        ui.visuals_mut().override_text_color = Some(theme::PALETTE_YELLOW());
        ui.vertical(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let button_label = if browser.handoff_resolution_pending {
                    "Handing back…"
                } else {
                    "Done — hand back to agent"
                };
                clicked = ui
                    .add_enabled(
                        !browser.handoff_resolution_pending,
                        egui::Button::new(RichText::new(button_label).size(11.0))
                            .fill(theme::blend(theme::PANEL_BG_ALT(), theme::PALETTE_YELLOW(), 0.2))
                            .corner_radius(6),
                    )
                    .clicked();
                ui.add(
                    egui::Label::new(RichText::new(format!("🖐 Agent paused: {reason}")).size(12.0))
                        .wrap_mode(TextWrapMode::Truncate),
                )
                .on_hover_text(reason);
            });
            if let Some(error) = &browser.handoff_error {
                ui.label(
                    RichText::new(format!("Could not hand back: {error}"))
                        .size(10.5)
                        .color(theme::PALETTE_RED()),
                );
            }
        });
    });
    if clicked {
        browser.hand_back();
    }
    clicked
}

//! Browser panel chrome strip: navigation buttons, URL bar, ownership
//! chip, and the handoff banner.

use egui::{RichText, Stroke, TextEdit, Ui, vec2};
use horizon_core::browser::{BrowserCommand, BrowserPanelState};

use crate::browser_widget::BrowserUiState;
use crate::theme;

const CHROME_HEIGHT: f32 = 30.0;

/// Draw the top strip. Returns `true` while the URL bar has focus.
pub fn show(
    ui: &mut Ui,
    panel_id: horizon_core::PanelId,
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    interactive: bool,
) -> bool {
    let chrome_row = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.set_min_height(CHROME_HEIGHT);
        nav_button(ui, "←", "Back", browser, BrowserCommand::Back, interactive);
        nav_button(ui, "→", "Forward", browser, BrowserCommand::Forward, interactive);
        nav_button(ui, "⟳", "Reload", browser, BrowserCommand::Reload, interactive);
        let url_focused = url_bar(ui, panel_id, browser, state, interactive);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ownership_chip(ui, browser);
        });
        url_focused
    });
    let url_focused = chrome_row.inner;

    let reason = browser.handoff_reason.clone();
    if let Some(reason) = reason {
        handoff_banner(ui, browser, &reason);
    }
    url_focused
}

fn nav_button(
    ui: &mut Ui,
    glyph: &str,
    label: &str,
    browser: &BrowserPanelState,
    command: BrowserCommand,
    interactive: bool,
) {
    let response = ui.add_enabled(
        interactive,
        egui::Button::new(RichText::new(glyph).size(13.0))
            .min_size(vec2(22.0, 22.0))
            .fill(theme::PANEL_BG_ALT())
            .corner_radius(6)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE())),
    );
    if response
        .on_hover_text_at_pointer(format!("{label} (browser)"))
        .clicked()
    {
        browser.send(command);
    }
}

fn url_bar(
    ui: &mut Ui,
    panel_id: horizon_core::PanelId,
    browser: &BrowserPanelState,
    state: &mut BrowserUiState,
    interactive: bool,
) -> bool {
    let id = ui.make_persistent_id(("browser-url-bar", panel_id));
    // Keep the buffer in sync with the live URL while unfocused.
    if ui.memory(|m| m.focused() != Some(id)) && state.url_buffer != browser.url {
        state.url_buffer.clone_from(&browser.url);
    }
    let response = ui.add_enabled(
        interactive,
        TextEdit::singleline(&mut state.url_buffer)
            .id(id)
            .hint_text("https://…")
            .desired_width(f32::INFINITY)
            .font(egui::FontId::monospace(11.5)),
    );
    // Enter submits while the bar is focused (egui singleline edits do
    // not consume it).
    if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        let url = state.url_buffer.trim().to_string();
        if !url.is_empty() && url != browser.url {
            browser.send(BrowserCommand::Navigate(url));
        }
    }
    response.has_focus()
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

fn handoff_banner(ui: &mut Ui, browser: &mut BrowserPanelState, reason: &str) {
    ui.scope(|ui| {
        ui.visuals_mut().override_text_color = Some(theme::PALETTE_YELLOW());
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(
                RichText::new(format!("🖐 Agent paused: {reason}")).size(12.0),
            ));
            if ui
                .add(
                    egui::Button::new(RichText::new("Done — hand back to agent").size(11.0))
                        .fill(theme::blend(theme::PANEL_BG_ALT(), theme::PALETTE_YELLOW(), 0.2))
                        .corner_radius(6),
                )
                .clicked()
            {
                browser.hand_back();
            }
        });
    });
}

//! Host-painted native `<select>` menu for browser panels.

use egui::{
    Align, Align2, Color32, CornerRadius, Layout, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2,
};
use horizon_core::browser::{BrowserCommand, BrowserPanelState, NativeSelectOption, NativeSelectPopup};

use crate::theme;

pub(super) const OPTION_HEIGHT: f32 = 28.0;
const MAX_MENU_HEIGHT: f32 = 280.0;
const MENU_PADDING: f32 = 4.0;

#[derive(Clone, Debug)]
pub(super) struct SelectPopupUi {
    pub(super) css_path: String,
    pub(super) highlight: usize,
    pub(super) typeahead: String,
    pub(super) typeahead_at: f64,
    /// Last highlight passed to `scroll_to_me`. Re-requesting every frame
    /// fights mouse-wheel browsing of a long list.
    scrolled_highlight: Option<usize>,
}

#[derive(Clone, Copy)]
pub(super) struct SelectMenuLayout {
    pub(super) menu: Rect,
}

impl SelectMenuLayout {
    #[must_use]
    pub(super) fn contains(self, pos: Pos2) -> bool {
        self.menu.contains(pos)
    }
}

#[must_use]
pub(super) fn menu_layout(
    image_rect: Rect,
    frame_size: [f32; 2],
    popup: &NativeSelectPopup,
) -> Option<SelectMenuLayout> {
    if frame_size[0] <= f32::EPSILON || frame_size[1] <= f32::EPSILON {
        return None;
    }
    let scale_x = image_rect.width() / frame_size[0];
    let scale_y = image_rect.height() / frame_size[1];
    #[allow(clippy::cast_possible_truncation)]
    let control = Rect::from_min_size(
        pos2(
            image_rect.left() + popup.bounds.x as f32 * scale_x,
            image_rect.top() + popup.bounds.y as f32 * scale_y,
        ),
        vec2(
            popup.bounds.width as f32 * scale_x,
            popup.bounds.height as f32 * scale_y,
        ),
    );
    let rows = popup.options.len().max(1);
    #[allow(clippy::cast_precision_loss)]
    let content_height = OPTION_HEIGHT * rows as f32;
    let height = (content_height + MENU_PADDING * 2.0)
        .min(MAX_MENU_HEIGHT)
        .min(image_rect.height());
    let width = control.width().max(168.0).min(image_rect.width());
    let mut left = control.left().max(image_rect.left());
    if left + width > image_rect.right() {
        left = (image_rect.right() - width).max(image_rect.left());
    }
    let below_top = control.bottom() + 2.0;
    let above_top = control.top() - height - 2.0;
    let top = if below_top + height <= image_rect.bottom() || above_top < image_rect.top() {
        below_top.min(image_rect.bottom() - height).max(image_rect.top())
    } else {
        above_top.max(image_rect.top())
    };
    Some(SelectMenuLayout {
        menu: Rect::from_min_size(pos2(left, top), vec2(width, height)),
    })
}

pub(super) fn sync_ui_state(state: &mut Option<SelectPopupUi>, popup: Option<&NativeSelectPopup>) {
    let Some(popup) = popup else {
        *state = None;
        return;
    };
    let selected = popup.selected_row();
    match state {
        Some(open) if open.css_path == popup.css_path => {
            if open.highlight >= popup.options.len() {
                open.highlight = selected;
                open.scrolled_highlight = None;
            }
        }
        _ => {
            *state = Some(SelectPopupUi {
                css_path: popup.css_path.clone(),
                highlight: selected,
                typeahead: String::new(),
                typeahead_at: 0.0,
                scrolled_highlight: None,
            });
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct SelectPopupKey {
    pub key: egui::Key,
    pub pressed: bool,
    pub modifiers: egui::Modifiers,
    pub now: f64,
    pub text: Option<char>,
}

pub(super) fn handle_key(
    browser: &BrowserPanelState,
    state: &mut Option<SelectPopupUi>,
    popup: &NativeSelectPopup,
    event: SelectPopupKey,
) -> bool {
    if !event.pressed {
        return false;
    }
    let Some(open) = state.as_mut() else {
        return false;
    };
    match event.key {
        egui::Key::Escape => {
            browser.send(BrowserCommand::NativeSelectDismiss);
            true
        }
        egui::Key::Enter | egui::Key::Space => {
            commit_highlight(browser, popup, open.highlight);
            true
        }
        egui::Key::ArrowDown if !event.modifiers.ctrl && !event.modifiers.mac_cmd => {
            open.highlight = step_highlight(popup, open.highlight, 1);
            true
        }
        egui::Key::ArrowUp if !event.modifiers.ctrl && !event.modifiers.mac_cmd => {
            open.highlight = step_highlight(popup, open.highlight, -1);
            true
        }
        egui::Key::Home => {
            open.highlight = first_enabled(popup).unwrap_or(open.highlight);
            true
        }
        egui::Key::End => {
            open.highlight = last_enabled(popup).unwrap_or(open.highlight);
            true
        }
        egui::Key::Tab => {
            commit_highlight(browser, popup, open.highlight);
            false
        }
        _ => {
            let Some(character) = event
                .text
                .filter(|character| character.is_alphanumeric() || *character == ' ')
            else {
                return false;
            };
            if event.modifiers.ctrl || event.modifiers.mac_cmd || event.modifiers.alt {
                return false;
            }
            if event.now - open.typeahead_at > 1.0 {
                open.typeahead.clear();
            }
            open.typeahead.push(character);
            open.typeahead_at = event.now;
            if let Some(index) = typeahead_index(popup, &open.typeahead) {
                open.highlight = index;
            }
            true
        }
    }
}

fn commit_highlight(browser: &BrowserPanelState, popup: &NativeSelectPopup, highlight: usize) {
    if let Some(option) = popup.options.get(highlight).filter(|option| !option.disabled) {
        browser.send(BrowserCommand::NativeSelectChoose { index: option.index });
    } else {
        browser.send(BrowserCommand::NativeSelectDismiss);
    }
}

fn step_highlight(popup: &NativeSelectPopup, highlight: usize, delta: i32) -> usize {
    let current = i32::try_from(highlight).unwrap_or(0);
    usize::try_from(popup.step_from(current, delta).max(0)).unwrap_or(highlight)
}

fn first_enabled(popup: &NativeSelectPopup) -> Option<usize> {
    popup.options.iter().position(|option| !option.disabled)
}

fn last_enabled(popup: &NativeSelectPopup) -> Option<usize> {
    popup.options.iter().rposition(|option| !option.disabled)
}

fn typeahead_index(popup: &NativeSelectPopup, prefix: &str) -> Option<usize> {
    let prefix = prefix.to_ascii_lowercase();
    popup
        .options
        .iter()
        .position(|option| !option.disabled && option.label.to_ascii_lowercase().starts_with(&prefix))
}

pub(super) fn show(
    ui: &mut Ui,
    browser: &BrowserPanelState,
    image_rect: Rect,
    frame_size: [f32; 2],
    popup: &NativeSelectPopup,
    open: &mut SelectPopupUi,
) -> Option<SelectMenuLayout> {
    let layout = menu_layout(image_rect, frame_size, popup)?;
    paint_menu_frame(ui, layout.menu);
    let inner = layout.menu.shrink(MENU_PADDING);
    ui.scope_builder(
        UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)),
        |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("native-select-options", browser.panel_local_id.as_str()))
                .max_height(inner.height())
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(inner.width());
                    let mut last_group: Option<&str> = None;
                    for (row, option) in popup.options.iter().enumerate() {
                        if option.group.as_deref() != last_group {
                            last_group = option.group.as_deref();
                            if let Some(group) = option.group.as_deref() {
                                paint_group_header(ui, inner.width(), group);
                            }
                        }
                        let response = paint_option_row(ui, inner.width(), option, row == open.highlight);
                        if row == open.highlight && open.scrolled_highlight != Some(open.highlight) {
                            response.scroll_to_me(Some(egui::Align::Center));
                            open.scrolled_highlight = Some(open.highlight);
                        }
                        if response.clicked() && !option.disabled {
                            browser.send(BrowserCommand::NativeSelectChoose { index: option.index });
                        }
                    }
                });
        },
    );
    Some(layout)
}

fn paint_menu_frame(ui: &Ui, rect: Rect) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(8), theme::PANEL_BG());
    painter.rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0, theme::alpha(theme::ACCENT(), 70)),
        StrokeKind::Outside,
    );
}

fn paint_group_header(ui: &mut Ui, width: f32, label: &str) {
    let (_, rect) = ui.allocate_space(vec2(width, 20.0));
    ui.painter().text(
        pos2(rect.left() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(11.0),
        theme::FG_DIM(),
    );
}

fn paint_option_row(ui: &mut Ui, width: f32, option: &NativeSelectOption, highlighted: bool) -> egui::Response {
    let (id, rect) = ui.allocate_space(vec2(width, OPTION_HEIGHT));
    let response = ui.interact(rect, id, Sense::click());
    let fill = if option.disabled {
        Color32::TRANSPARENT
    } else if highlighted || response.hovered() {
        theme::alpha(theme::ACCENT(), 40)
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter()
            .rect_filled(rect.shrink2(vec2(2.0, 1.0)), CornerRadius::same(4), fill);
    }
    let color = if option.disabled {
        theme::FG_DIM()
    } else if option.selected {
        theme::ACCENT()
    } else {
        theme::FG()
    };
    ui.painter().text(
        pos2(rect.left() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        &option.label,
        egui::FontId::proportional(13.0),
        color,
    );
    response
}

#[cfg(test)]
mod tests {
    use horizon_core::browser::{BrowserBounds, NativeSelectOption, NativeSelectPopup};

    use super::{MAX_MENU_HEIGHT, menu_layout, sync_ui_state};
    use egui::{Rect, pos2};

    fn popup_at(y: f64, option_count: usize) -> NativeSelectPopup {
        NativeSelectPopup {
            css_path: "#native-single".to_string(),
            name: "native-single".to_string(),
            selected_index: 0,
            bounds: BrowserBounds {
                x: 20.0,
                y,
                width: 80.0,
                height: 22.0,
            },
            options: (0..option_count)
                .map(|index| NativeSelectOption {
                    index: u32::try_from(index).unwrap_or(0),
                    value: index.to_string(),
                    label: format!("Item {index}"),
                    group: None,
                    disabled: false,
                    selected: index == 0,
                })
                .collect(),
        }
    }

    #[test]
    fn menu_opens_below_when_there_is_room_and_flips_near_the_bottom() {
        let image = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 300.0));
        let below = menu_layout(image, [400.0, 300.0], &popup_at(40.0, 3)).expect("below");
        assert!(below.menu.top() > 62.0);
        assert!(below.menu.height() < MAX_MENU_HEIGHT);
        let long = menu_layout(image, [400.0, 300.0], &popup_at(40.0, 40)).expect("scrollable");
        assert!((long.menu.height() - MAX_MENU_HEIGHT).abs() < 0.1);
        let edge_popup = popup_at(270.0, 3);
        let edge = menu_layout(image, [400.0, 300.0], &edge_popup).expect("flipped");
        let control_top = 270.0_f32;
        assert!(edge.menu.bottom() <= control_top + 22.0 + 1.0);
        assert!(edge.menu.top() < control_top);
    }

    #[test]
    fn sync_ui_state_highlights_the_selected_row_inside_a_windowed_list() {
        let popup = NativeSelectPopup {
            css_path: "#native-long".to_string(),
            name: "native-long".to_string(),
            selected_index: 550,
            bounds: BrowserBounds {
                x: 20.0,
                y: 40.0,
                width: 80.0,
                height: 22.0,
            },
            options: (548u32..552)
                .map(|index| NativeSelectOption {
                    index,
                    value: index.to_string(),
                    label: format!("Item {index}"),
                    group: None,
                    disabled: false,
                    selected: index == 550,
                })
                .collect(),
        };
        let mut state = None;
        sync_ui_state(&mut state, Some(&popup));
        assert_eq!(state.expect("open").highlight, 2);
    }
}

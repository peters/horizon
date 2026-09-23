//! Interact: forwarding a person's pointer and keyboard to the desktop while
//! the image has focus, and letting go of everything when it does not.
use egui::Ui;
use horizon_core::DeviceImageLayout;

use super::DeviceUiState;
use super::input::{InputState, SCROLL_NOTCH_POINTS, button_bit, desktop_point};
use vnc::X11Event;

impl DeviceUiState {
    pub(super) fn interact_toggle(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let was = self.interact;
            ui.toggle_value(&mut self.interact, "Interact").on_hover_text(
                "Send your mouse and keyboard to the desktop while the image has focus. \
                 Click the image to start; click outside it or turn this off to stop. \
                 Off by default, and never turned on by agents.",
            );
            if was && !self.interact {
                self.release_input();
            }
            if self.interact {
                ui.weak(if self.captured {
                    "Keyboard captured, Escape included; click outside the image to release it"
                } else {
                    "Click the image to type into it"
                });
            }
        });
    }

    /// The crop and scale the current image was made with, for mapping points.
    fn image_layout(&self) -> Option<DeviceImageLayout> {
        let desktop = self.desktop?;
        self.controls.options.for_desktop(desktop).layout(desktop).ok()
    }

    pub(super) fn forward_input(&mut self, ui: &Ui, response: &egui::Response) {
        let Some(layout) = self.image_layout() else {
            return;
        };
        if response.hovered() && ui.input(|input| input.pointer.any_pressed()) {
            response.request_focus();
        }
        let mut events = self.pointer_input(ui, response, &layout);
        if response.has_focus() {
            events.extend(self.keyboard_input(ui, response));
            self.captured = true;
        } else if self.captured {
            events.extend(self.input.release_all());
            self.captured = false;
        }
        if let Some(session) = &self.session {
            session.send_input(events);
        }
    }

    /// Pointer input is replayed per event rather than sampled once per frame:
    /// a click whose press and release land in one frame, or several moves
    /// coalesced by a slow repaint, must all reach the desktop.
    fn pointer_input(&mut self, ui: &Ui, response: &egui::Response, layout: &DeviceImageLayout) -> Vec<X11Event> {
        let (pointer_events, scroll) = ui.input(|input| {
            let pointer: Vec<egui::Event> = input
                .events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } | egui::Event::PointerGone
                    )
                })
                .cloned()
                .collect();
            (pointer, wheel_travel(input))
        });
        // Event positions are global; the image rect lives in this panel's
        // layer, which the canvas pans and zooms.
        let from_global = ui.ctx().layer_transform_from_global(ui.layer_id());
        let mapped = |pos: egui::Pos2| {
            let local = from_global.map_or(pos, |transform| transform * pos);
            desktop_point(response.rect, layout, local)
        };
        let mut events = Vec::new();
        for event in pointer_events {
            match event {
                egui::Event::PointerMoved(pos) => {
                    // Over the image only while egui routes the pointer here, so
                    // hovering a popup or panel on top does not move the desktop.
                    let inside = mapped(pos).filter(|_| response.hovered());
                    if inside.is_some() || self.input.buttons() != 0 {
                        let buttons = self.input.buttons();
                        events.extend(self.input.pointer(inside, buttons));
                    }
                }
                egui::Event::PointerButton {
                    pos, button, pressed, ..
                } => {
                    let Some(bit) = button_bit(button) else {
                        continue;
                    };
                    let inside = mapped(pos);
                    // Presses start on the image and only when no other layer
                    // or widget is on top of it; releases end a drag anywhere.
                    let press = pressed && inside.is_some() && response.hovered();
                    if press || (!pressed && self.input.buttons() & bit != 0) {
                        let buttons = if pressed {
                            self.input.buttons() | bit
                        } else {
                            self.input.buttons() & !bit
                        };
                        events.extend(self.input.pointer(inside, buttons));
                    }
                }
                // The pointer left the window; a release there never reaches
                // us, so end any drag at the last position the desktop saw.
                egui::Event::PointerGone if self.input.buttons() != 0 => {
                    events.extend(self.input.pointer(None, 0));
                }
                _ => {}
            }
        }
        if response.hovered() {
            events.extend(self.input.scroll(scroll));
        }
        events
    }

    /// Every key belongs to the desktop while it is captured, Escape
    /// included; clicking outside the image or turning Interact off gives the
    /// keyboard back.
    fn keyboard_input(&mut self, ui: &Ui, response: &egui::Response) -> Vec<X11Event> {
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            );
        });
        let (modifiers, raw_events) = ui.input(|input| (input.modifiers, input.events.clone()));
        let mut events = Vec::new();
        for event in raw_events {
            match event {
                egui::Event::Key {
                    key,
                    pressed,
                    modifiers,
                    ..
                } => {
                    // The key's own modifier snapshot comes first, so a chord
                    // reaches the desktop with its modifier already down.
                    events.extend(self.input.modifiers(modifiers));
                    events.extend(self.input.key(key, pressed, modifiers));
                }
                egui::Event::Text(text) => events.extend(InputState::text(&text)),
                _ => {}
            }
        }
        events.extend(self.input.modifiers(modifiers));
        events
    }

    /// Let go of every key and button the desktop still thinks are held.
    pub(super) fn release_input(&mut self) {
        let events = self.input.release_all();
        self.captured = false;
        if let Some(session) = &self.session
            && !events.is_empty()
        {
            session.send_input(events);
        }
    }
}

/// This frame's wheel travel in points, from raw events rather than egui's
/// smoothed delta, which hands out only a fraction of it per repaint.
fn wheel_travel(input: &egui::InputState) -> egui::Vec2 {
    input
        .events
        .iter()
        .filter_map(|event| match event {
            egui::Event::MouseWheel { unit, delta, .. } => Some(match unit {
                egui::MouseWheelUnit::Point => *delta,
                egui::MouseWheelUnit::Line => *delta * SCROLL_NOTCH_POINTS,
                egui::MouseWheelUnit::Page => *delta * SCROLL_NOTCH_POINTS * 3.0,
            }),
            _ => None,
        })
        .fold(egui::Vec2::ZERO, |sum, delta| sum + delta)
}

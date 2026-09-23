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
        // Widget focus survives the window losing OS focus (Alt-Tab), after
        // which no key-up is guaranteed; treat that as the end of capture.
        let window_focused = ui.input(|input| input.viewport().focused.unwrap_or(true));
        if window_focused && response.has_focus() {
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
        let pointer_events: Vec<egui::Event> = ui.input(|input| {
            input
                .events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        egui::Event::PointerMoved(_)
                            | egui::Event::PointerButton { .. }
                            | egui::Event::PointerGone
                            | egui::Event::MouseWheel { .. }
                    )
                })
                .cloned()
                .collect()
        });
        // Event positions are global; the image rect lives in this panel's
        // layer, which the canvas pans and zooms.
        let from_global = ui.ctx().layer_transform_from_global(ui.layer_id());
        let mapped = |pos: egui::Pos2| {
            let local = from_global.map_or(pos, |transform| transform * pos);
            desktop_point(response.rect, layout, local)
        };
        // Whether egui gave this frame's pointer gesture to the image, the same
        // test the browser panel uses.
        let owns_pointer = response.contains_pointer()
            || response.is_pointer_button_down_on()
            || response.drag_started()
            || response.dragged()
            || response.interact_pointer_pos().is_some();
        // egui judges a gesture by where the pointer ended the frame, so a
        // press that left the image before the repaint is decided by the
        // layer on top at the press position instead: this panel's layer, not
        // a popup, modal or overlapping panel.
        let this_layer = ui.layer_id();
        let on_top_at =
            |global: egui::Pos2| ui.ctx().layer_id_at(global).unwrap_or_else(egui::LayerId::background) == this_layer;
        let mut events = Vec::new();
        if self.pointer_global.is_none() {
            // First frame of a capture (Interact just turned on, or the viewer
            // drawn again): the best known position is the current one.
            self.pointer_global = ui.input(|input| input.pointer.latest_pos());
        }
        for event in pointer_events {
            if let egui::Event::PointerMoved(pos) | egui::Event::PointerButton { pos, .. } = event {
                self.pointer_global = Some(pos);
            }
            match event {
                egui::Event::PointerMoved(pos) => {
                    // Over the image only while this panel is on top there, so
                    // hovering a popup or panel on top does not move the desktop.
                    let inside = mapped(pos).filter(|_| owns_pointer || on_top_at(pos));
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
                    let press = pressed && inside.is_some() && (owns_pointer || on_top_at(pos));
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
                egui::Event::PointerGone => {
                    self.pointer_global = None;
                    if self.input.buttons() != 0 {
                        events.extend(self.input.pointer(None, 0));
                    }
                }
                // A wheel event belongs to whatever was under the pointer when
                // it happened, not where the pointer ended the frame.
                egui::Event::MouseWheel { unit, delta, .. } => {
                    let over_image = self
                        .pointer_global
                        .is_some_and(|pos| mapped(pos).is_some() && on_top_at(pos));
                    if over_image {
                        events.extend(self.input.scroll(wheel_points(unit, delta)));
                    }
                }
                _ => {}
            }
        }
        // The next frame's first wheel may arrive without a move; start it
        // from where the pointer is now.
        if let Some(latest) = ui.input(|input| input.pointer.latest_pos()) {
            self.pointer_global = Some(latest);
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
        publish_ime_output(ui, response.rect);
        let (modifiers, raw_events) = ui.input(|input| (input.modifiers, input.events.clone()));
        let mut events = Vec::new();
        for event in raw_events {
            // egui-winit turns the Ctrl or Command press of C, X and V into
            // these instead of a key event (the release still arrives as a
            // key). Send the chord so the remote application runs its own
            // clipboard action; the local clipboard text in Paste is never
            // typed into the remote desktop.
            let clipboard_letter = match &event {
                egui::Event::Copy => Some('c'),
                egui::Event::Cut => Some('x'),
                egui::Event::Paste(_) => Some('v'),
                _ => None,
            };
            if let Some(letter) = clipboard_letter {
                events.extend(self.input.clipboard_chord(letter));
                continue;
            }
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
                egui::Event::Text(text) | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                    events.extend(InputState::text(&text));
                }
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
        self.pointer_global = None;
        if let Some(session) = &self.session
            && !events.is_empty()
        {
            session.send_input(events);
        }
    }
}

/// Enable the platform input method over the captured image, so composed
/// text (CJK and similar) arrives as a commit instead of being unavailable.
fn publish_ime_output(ui: &Ui, image: egui::Rect) {
    let to_global = ui.ctx().layer_transform_to_global(ui.layer_id()).unwrap_or_default();
    let cursor = egui::Rect::from_min_size(image.min, egui::vec2(1.0, 1.0));
    ui.ctx().output_mut(|output| {
        output.ime = Some(egui::output::IMEOutput {
            purpose: egui::IMEPurpose::Normal,
            rect: to_global * image,
            cursor_rect: to_global * cursor,
            should_interrupt_composition: false,
        });
    });
}

/// One wheel event's travel in points, from raw events rather than egui's
/// smoothed delta, which hands out only a fraction of it per repaint.
fn wheel_points(unit: egui::MouseWheelUnit, delta: egui::Vec2) -> egui::Vec2 {
    match unit {
        egui::MouseWheelUnit::Point => delta,
        egui::MouseWheelUnit::Line => delta * SCROLL_NOTCH_POINTS,
        egui::MouseWheelUnit::Page => delta * SCROLL_NOTCH_POINTS * 3.0,
    }
}

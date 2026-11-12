//! Interact for a frame source program: a person's pointer and keyboard as the
//! `input` messages of the contract, in the pixels of the picture.
//!
//! The mapping of an egui key is the one of the VNC viewer
//! ([`super::keys::keysym_for_key`]): the keysym of a key becomes the name of the
//! contract through the one table of
//! [`horizon_frame_source::keys`], so both viewers send one name for one key. What
//! a person types goes as `text`, so a character is right with two keyboard
//! layouts, and a character that is pressed with Ctrl, Alt or Super goes as `key`.
use egui::Ui;
use horizon_frame_source::{InputState, Key, Point, keys::NamedKey};
use horizon_vnc::ImageLayout;

use super::DeviceUiState;
use super::keys::{desktop_point, keysym_for_key, modifiers};

impl DeviceUiState {
    /// Forward the input of this frame to the program, in the pixels of the
    /// picture that the widget shows.
    pub(super) fn forward_frame_input(&mut self, ui: &Ui, response: &egui::Response, layout: &ImageLayout) {
        Self::focus_picture(ui, response);
        let mut events = self.frame_source_pointer(ui, response, layout);
        let window_focused = ui.input(|input| input.viewport().focused.unwrap_or(true));
        if window_focused && response.has_focus() {
            events.extend(self.frame_source_keyboard(ui, response));
            self.captured = true;
        } else if self.captured {
            // The program lets go of everything itself when it is told that the
            // viewer is hidden, and this is the earlier message of the two.
            events.extend(self.frame_state.release_all());
            self.captured = false;
        }
        self.send_frame_input(events);
    }

    /// A press inside the picture takes the keyboard, and a press outside it gives
    /// the keyboard back, as the VNC viewer does. A viewer that never takes the
    /// keyboard never sends a key: the picture is the only target of one.
    fn focus_picture(ui: &Ui, response: &egui::Response) {
        let press = ui.input(|input| {
            input
                .pointer
                .any_pressed()
                .then(|| input.pointer.interact_pos())
                .flatten()
        });
        let Some(position) = press else {
            return;
        };
        let local = ui
            .ctx()
            .layer_transform_from_global(response.layer_id)
            .map_or(position, |transform| transform * position);
        if response.rect.contains(local) && ui.ctx().layer_id_at(position) == Some(response.layer_id) {
            response.request_focus();
        } else {
            response.surrender_focus();
        }
    }

    fn send_frame_input(&mut self, events: Vec<horizon_frame_source::InputEvent>) {
        if let Some(session) = &self.session {
            session.send_frame_input(events);
        }
    }

    /// Every pointer event of this frame, replayed in its own order, so a click
    /// whose press and release land in one frame reaches the program.
    fn frame_source_pointer(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        layout: &ImageLayout,
    ) -> Vec<horizon_frame_source::InputEvent> {
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
        let from_global = ui.ctx().layer_transform_from_global(ui.layer_id());
        let mapped = |pos: egui::Pos2| {
            let local = from_global.map_or(pos, |transform| transform * pos);
            desktop_point(response.rect, layout, local).map(|point| Point {
                x: u32::from(point.x),
                y: u32::from(point.y),
            })
        };
        // An event belongs to this viewer only where this layer is the one on top,
        // so a popup, a dialog or another panel above the picture takes its own
        // events, as it does for the VNC viewer.
        let this_layer = ui.layer_id();
        let on_top_at =
            |global: egui::Pos2| ui.ctx().layer_id_at(global).unwrap_or_else(egui::LayerId::background) == this_layer;
        let mut events = Vec::new();
        if self.pointer_global.is_none() {
            // The first frame of a capture, or a viewer that was drawn again: the
            // best known position is the current one.
            self.pointer_global = ui.input(|input| input.pointer.latest_pos());
        }
        for event in pointer_events {
            if let egui::Event::PointerMoved(pos) | egui::Event::PointerButton { pos, .. } = event {
                self.pointer_global = Some(pos);
            }
            match event {
                egui::Event::PointerMoved(pos) => {
                    let inside = mapped(pos).filter(|_| on_top_at(pos));
                    events.extend(self.frame_state.pointer(inside));
                }
                egui::Event::PointerButton {
                    pos, button, pressed, ..
                } => {
                    let Some(button) = super::keys::button_bit(button).and_then(horizon_frame_source::Button::of_bit)
                    else {
                        continue;
                    };
                    // A press starts in the picture and only under this layer; a
                    // release ends the press that this program was given.
                    let inside = if pressed {
                        mapped(pos).filter(|_| on_top_at(pos))
                    } else {
                        mapped(pos)
                    };
                    events.extend(self.frame_state.button(button, pressed, inside));
                }
                egui::Event::PointerGone => {
                    events.extend(self.frame_state.button(horizon_frame_source::Button::Left, false, None));
                    events.extend(
                        self.frame_state
                            .button(horizon_frame_source::Button::Middle, false, None),
                    );
                    events.extend(
                        self.frame_state
                            .button(horizon_frame_source::Button::Right, false, None),
                    );
                }
                // A wheel event belongs to the picture under the pointer when it
                // happened, and one notch is one step of the contract.
                egui::Event::MouseWheel { unit, delta, .. } => {
                    let at = self.pointer_global.filter(|pos| on_top_at(*pos)).and_then(mapped);
                    if let Some(point) = at {
                        events.extend(self.frame_state.pointer(Some(point)));
                        let (dx, dy) = self.frame_wheel.steps(unit, delta);
                        events.extend(self.frame_state.wheel(dx, dy));
                    }
                }
                _ => {}
            }
        }
        events
    }

    /// The keys and the text of this frame.
    fn frame_source_keyboard(&mut self, ui: &Ui, response: &egui::Response) -> Vec<horizon_frame_source::InputEvent> {
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
        let raw_events = ui.input(|input| input.events.clone());
        let mut events = Vec::new();
        // The program reads what the person typed as one message of this frame, so
        // the characters of a frame are collected and sent together, in their place
        // among the keys.
        let mut typed = String::new();
        let flush = |typed: &mut String, events: &mut Vec<horizon_frame_source::InputEvent>| {
            if !typed.is_empty() {
                events.extend(InputState::text(typed));
                typed.clear();
            }
        };
        for event in raw_events {
            // egui-winit reports the platform chord of C, X and V as these instead
            // of a key press. The program gets the chord as its keys, and the local
            // clipboard text of Paste is never typed into it.
            let clipboard_letter = match &event {
                egui::Event::Copy => Some('c'),
                egui::Event::Cut => Some('x'),
                egui::Event::Paste(_) => Some('v'),
                _ => None,
            };
            if let Some(letter) = clipboard_letter {
                flush(&mut typed, &mut events);
                events.extend(self.frame_chord(letter));
                continue;
            }
            match event {
                egui::Event::Key {
                    key,
                    pressed,
                    modifiers: at,
                    ..
                } => {
                    flush(&mut typed, &mut events);
                    let chord_modifier = at.ctrl || at.alt || at.mac_cmd;
                    events.extend(self.modifier_keys(at));
                    let keysym = keysym_for_key(key);
                    let named = keysym.and_then(Key::of_keysym);
                    match (named, chord_modifier) {
                        (Some(key), _) => {
                            // A release follows its press and no other: a press that
                            // Horizon handled as its own shortcut never reached the
                            // program, and its release must not reach it either.
                            if pressed || self.frame_state.is_held(key) {
                                events.extend(self.frame_state.key(key, pressed));
                            }
                        }
                        (None, true) => {
                            if let Some(character) = keysym.and_then(character_of_keysym) {
                                events.extend(self.frame_state.key(Key::Character(character), pressed));
                            }
                        }
                        // A character without Ctrl, Alt or Super arrives again as
                        // text, so the text is the one key of it.
                        (None, false) => {}
                    }
                }
                egui::Event::Text(text) | egui::Event::Ime(egui::ImeEvent::Commit(text)) => typed.push_str(&text),
                _ => {}
            }
        }
        flush(&mut typed, &mut events);
        // The modifier keys of the frame, as the frame ended it, so a key that was
        // pressed with Ctrl reaches the program with Ctrl down and no modifier
        // stays down after the frame.
        let at = ui.input(|input| input.modifiers);
        events.extend(self.modifier_keys(at));
        events
    }

    /// One clipboard letter as the chord of the platform: the command key held
    /// around the letter, so the program runs its own clipboard action.
    fn frame_chord(&mut self, letter: char) -> Vec<horizon_frame_source::InputEvent> {
        let command = if cfg!(target_os = "macos") {
            NamedKey::Super
        } else {
            NamedKey::Ctrl
        };
        self.frame_state.chord(letter, command)
    }

    /// The four modifier keys as the names of the contract, sent as their changes.
    fn modifier_keys(&mut self, at: egui::Modifiers) -> Vec<horizon_frame_source::InputEvent> {
        let wanted = modifiers(at);
        let mut events = Vec::new();
        for (name, down) in [
            (NamedKey::Shift, wanted.shift),
            (NamedKey::Ctrl, wanted.ctrl),
            (NamedKey::Alt, wanted.alt),
            (NamedKey::Super, wanted.command),
        ] {
            let key = Key::Named(name);
            if self.frame_state.is_held(key) != down {
                events.extend(self.frame_state.key(key, down));
            }
        }
        events
    }
}

/// The character of one keysym: Latin-1 is its own code point, and the rest is the
/// Unicode range of X11.
fn character_of_keysym(keysym: u32) -> Option<char> {
    let code = if keysym >= horizon_vnc::input::UNICODE_KEYSYM_BASE {
        keysym - horizon_vnc::input::UNICODE_KEYSYM_BASE
    } else if keysym < 0xff00 {
        keysym
    } else {
        // A keysym of the function range above Latin-1 is a key that the table of
        // names holds, and no character that a person types.
        return None;
    };
    let character = char::from_u32(code)?;
    (!character.is_control()).then_some(character)
}

#[cfg(test)]
mod tests {
    use super::character_of_keysym;

    #[test]
    fn a_keysym_becomes_the_character_that_a_chord_names() {
        assert_eq!(character_of_keysym(u32::from('a')), Some('a'));
        assert_eq!(character_of_keysym(u32::from('7')), Some('7'));
        assert_eq!(
            character_of_keysym(horizon_vnc::input::UNICODE_KEYSYM_BASE | u32::from('ø')),
            Some('ø')
        );
        assert_eq!(character_of_keysym(u32::from('\n')), None);
        // A keysym of the function range above Latin-1 is no character that a person
        // types, and the caller sends the key of the table for it first.
        assert_eq!(
            character_of_keysym(0xff0d),
            None,
            "a key of the function range is no character"
        );
    }
}

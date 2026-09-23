//! Turning the viewer's pointer and keyboard into VNC input while a person has
//! Interact on. Nothing here runs for agent-created viewers unless a person
//! turns the toggle on; the panel stays a read-only view by default.
use egui::{Key, Modifiers, PointerButton, Pos2, Rect};
use horizon_core::DeviceImageLayout;
use vnc::{ClientKeyEvent, ClientMouseEvent, X11Event};

// X11 keysyms; VNC KeyEvent carries them verbatim.
const XK_BACKSPACE: u32 = 0xff08;
const XK_TAB: u32 = 0xff09;
const XK_RETURN: u32 = 0xff0d;
const XK_ESCAPE: u32 = 0xff1b;
const XK_HOME: u32 = 0xff50;
const XK_LEFT: u32 = 0xff51;
const XK_UP: u32 = 0xff52;
const XK_RIGHT: u32 = 0xff53;
const XK_DOWN: u32 = 0xff54;
const XK_PAGE_UP: u32 = 0xff55;
const XK_PAGE_DOWN: u32 = 0xff56;
const XK_END: u32 = 0xff57;
const XK_INSERT: u32 = 0xff63;
const XK_F1: u32 = 0xffbe;
const XK_SHIFT_L: u32 = 0xffe1;
const XK_CONTROL_L: u32 = 0xffe3;
const XK_ALT_L: u32 = 0xffe9;
const XK_SUPER_L: u32 = 0xffeb;
const XK_DELETE: u32 = 0xffff;
/// Keysyms above Latin-1 are the Unicode code point with this bit set.
const UNICODE_KEYSYM_BASE: u32 = 0x0100_0000;

/// RFB pointer button mask bits.
const BUTTON_LEFT: u8 = 1;
const BUTTON_MIDDLE: u8 = 2;
const BUTTON_RIGHT: u8 = 4;
const BUTTON_SCROLL_UP: u8 = 8;
const BUTTON_SCROLL_DOWN: u8 = 16;
const BUTTON_SCROLL_LEFT: u8 = 32;
const BUTTON_SCROLL_RIGHT: u8 = 64;
/// One scroll notch per this many logical points of wheel travel.
pub(super) const SCROLL_NOTCH_POINTS: f32 = 40.0;
const MAX_SCROLL_NOTCHES_PER_FRAME: u32 = 8;

/// Where the pointer is on the desktop, in server pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DesktopPoint {
    pub x: u16,
    pub y: u16,
}

/// Map a point inside the rendered image back to the desktop pixel it shows,
/// honouring the active crop and scaling. Outside the image is `None`.
pub(super) fn desktop_point(image: Rect, layout: &DeviceImageLayout, pos: Pos2) -> Option<DesktopPoint> {
    if !image.contains(pos) || image.width() <= 0.0 || image.height() <= 0.0 {
        return None;
    }
    let source = layout.source;
    let relative_x = ((pos.x - image.min.x) / image.width()).clamp(0.0, 1.0);
    let relative_y = ((pos.y - image.min.y) / image.height()).clamp(0.0, 1.0);
    let axis = |relative: f32, start: usize, span: usize| {
        let last = span.saturating_sub(1);
        // Desktops are bounded to 8192 px, well inside f32's exact integers.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let offset = ((relative * span as f32) as usize).min(last);
        u16::try_from(start + offset).unwrap_or(u16::MAX)
    };
    Some(DesktopPoint {
        x: axis(relative_x, source.x, source.width),
        y: axis(relative_y, source.y, source.height),
    })
}

/// What the viewer has told the server so far, so only changes are sent and
/// every pressed key or button is released when capture ends.
#[derive(Debug, Default)]
pub(super) struct InputState {
    buttons: u8,
    position: Option<DesktopPoint>,
    modifiers: Modifiers,
    held_keys: Vec<u32>,
    /// Wheel travel not yet worth a whole notch; trackpads deliver many small
    /// deltas that must add up rather than each become a click.
    scroll_remainder: egui::Vec2,
    /// Clipboard chords already sent whose key release is still to come.
    pending_clipboard: Vec<char>,
    /// egui-winit reported a V press since the last V release. It reports one
    /// for every V except command+V, so a release without one was a paste.
    v_pressed: bool,
}

impl InputState {
    /// The button mask the desktop currently believes is held.
    pub(super) fn buttons(&self) -> u8 {
        self.buttons
    }

    /// Pointer events for this frame: a move when the position changed and a
    /// button event when the mask changed. `position` is `None` off the image.
    pub(super) fn pointer(&mut self, position: Option<DesktopPoint>, buttons: u8) -> Vec<X11Event> {
        let mut events = Vec::new();
        let Some(position) = position.or(self.position) else {
            return events;
        };
        if Some(position) != self.position || buttons != self.buttons {
            self.position = Some(position);
            self.buttons = buttons;
            events.push(mouse(position, buttons));
        }
        events
    }

    /// Wheel travel as scroll button clicks at the current pointer position.
    /// Travel is accumulated across frames and only whole notches are sent;
    /// the part beyond the per-frame cap is dropped rather than queued.
    pub(super) fn scroll(&mut self, delta: egui::Vec2) -> Vec<X11Event> {
        let Some(position) = self.position else {
            return Vec::new();
        };
        self.scroll_remainder += delta;
        let mut events = Vec::new();
        for (axis, positive, negative) in [
            (1, BUTTON_SCROLL_UP, BUTTON_SCROLL_DOWN),
            (0, BUTTON_SCROLL_LEFT, BUTTON_SCROLL_RIGHT),
        ] {
            let travel = self.scroll_remainder[axis];
            let whole = (travel / SCROLL_NOTCH_POINTS).trunc();
            if whole == 0.0 {
                continue;
            }
            self.scroll_remainder[axis] = travel - whole * SCROLL_NOTCH_POINTS;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let notches = (whole.abs() as u32).min(MAX_SCROLL_NOTCHES_PER_FRAME);
            let button = if whole > 0.0 { positive } else { negative };
            for _ in 0..notches {
                events.push(mouse(position, self.buttons | button));
                events.push(mouse(position, self.buttons));
            }
        }
        events
    }

    /// Modifier keysyms for a change of the logical modifier state; egui does
    /// not report Shift or Ctrl presses on every platform, so the state is
    /// diffed rather than trusting per-key events.
    pub(super) fn modifiers(&mut self, modifiers: Modifiers) -> Vec<X11Event> {
        let mut events = Vec::new();
        for (was, now, keysym) in [
            (self.modifiers.shift, modifiers.shift, XK_SHIFT_L),
            (self.modifiers.ctrl, modifiers.ctrl, XK_CONTROL_L),
            (self.modifiers.alt, modifiers.alt, XK_ALT_L),
            (self.modifiers.mac_cmd, modifiers.mac_cmd, XK_SUPER_L),
        ] {
            if was != now {
                events.push(key(keysym, now));
            }
        }
        self.modifiers = modifiers;
        events
    }

    /// A key press or release. Printable keys without Ctrl or Command arrive
    /// again as text, so they are left to [`Self::text`]; everything else is
    /// sent as its keysym.
    pub(super) fn key(&mut self, pressed: Key, down: bool, modifiers: Modifiers) -> Vec<X11Event> {
        let Some(keysym) = keysym_for_key(pressed) else {
            return Vec::new();
        };
        if pressed == Key::V && down {
            self.v_pressed = true;
        }
        if !down && let Some(events) = self.clipboard_release(pressed) {
            return events;
        }
        let printable = keysym < 0xff00;
        // A release always follows its press, even if the modifier that made
        // the press a chord went up first; otherwise the key stays held.
        let releasing_held = !down && self.held_keys.contains(&keysym);
        if printable && !(modifiers.ctrl || modifiers.mac_cmd) && !releasing_held {
            return Vec::new();
        }
        self.track(keysym, down);
        vec![key(keysym, down)]
    }

    /// A clipboard shortcut egui-winit reported as Copy, Cut or Paste: the
    /// letter with the platform command modifier held around it, whatever
    /// the modifier state was by the end of the frame. The key's release,
    /// which egui-winit still reports, is then swallowed.
    pub(super) fn clipboard_chord(&mut self, letter: char) -> Vec<X11Event> {
        let mut chord = self.modifiers;
        if cfg!(target_os = "macos") {
            chord.mac_cmd = true;
        } else {
            chord.ctrl = true;
        }
        let mut events = self.modifiers(chord);
        events.extend(Self::tap(letter));
        self.pending_clipboard.push(letter);
        events
    }

    /// One press and release of a letter, for a chord whose modifier state
    /// has already been sent.
    pub(super) fn tap(letter: char) -> Vec<X11Event> {
        let keysym = keysym_for_char(letter);
        vec![key(keysym, true), key(keysym, false)]
    }

    /// The release of C, X or V after a clipboard chord is swallowed. A V
    /// release with neither a V press nor a Paste before it was command+V
    /// with an empty local clipboard, which egui-winit reports as nothing but
    /// that release (often after the modifier's own release); the chord is
    /// sent now.
    fn clipboard_release(&mut self, pressed: Key) -> Option<Vec<X11Event>> {
        let letter = match pressed {
            Key::C => 'c',
            Key::X => 'x',
            Key::V => 'v',
            _ => return None,
        };
        let v_pressed = std::mem::take(&mut self.v_pressed) || letter != 'v';
        if let Some(index) = self.pending_clipboard.iter().position(|pending| *pending == letter) {
            self.pending_clipboard.remove(index);
            return Some(Vec::new());
        }
        if !v_pressed {
            let events = self.clipboard_chord('v');
            self.pending_clipboard.pop();
            // The chord's modifier goes back to the frame's state at its end.
            return Some(events);
        }
        None
    }

    /// Typed text: a press and release per character.
    pub(super) fn text(text: &str) -> Vec<X11Event> {
        text.chars()
            .filter(|character| !character.is_control())
            .flat_map(|character| {
                let keysym = keysym_for_char(character);
                [key(keysym, true), key(keysym, false)]
            })
            .collect()
    }

    /// Release everything still held when the viewer loses capture, so the
    /// desktop is not left with a stuck modifier or mouse button.
    pub(super) fn release_all(&mut self) -> Vec<X11Event> {
        // A partial wheel gesture must not carry into the next capture.
        self.scroll_remainder = egui::Vec2::ZERO;
        self.pending_clipboard.clear();
        self.v_pressed = false;
        let mut events = Vec::new();
        for keysym in self.held_keys.drain(..) {
            events.push(key(keysym, false));
        }
        events.extend(self.modifiers(Modifiers::NONE));
        if self.buttons != 0
            && let Some(position) = self.position
        {
            self.buttons = 0;
            events.push(mouse(position, 0));
        }
        events
    }

    fn track(&mut self, keysym: u32, down: bool) {
        if down {
            if !self.held_keys.contains(&keysym) {
                self.held_keys.push(keysym);
            }
        } else {
            self.held_keys.retain(|held| *held != keysym);
        }
    }
}

/// The RFB mask bit for one of egui's pointer buttons; extra buttons have none.
pub(super) const fn button_bit(button: PointerButton) -> Option<u8> {
    match button {
        PointerButton::Primary => Some(BUTTON_LEFT),
        PointerButton::Secondary => Some(BUTTON_RIGHT),
        PointerButton::Middle => Some(BUTTON_MIDDLE),
        PointerButton::Extra1 | PointerButton::Extra2 => None,
    }
}

fn key(keysym: u32, down: bool) -> X11Event {
    X11Event::KeyEvent(ClientKeyEvent { keycode: keysym, down })
}

fn mouse(position: DesktopPoint, buttons: u8) -> X11Event {
    X11Event::PointerEvent(ClientMouseEvent {
        position_x: position.x,
        position_y: position.y,
        bottons: buttons,
    })
}

/// Latin-1 characters are their own keysym; the rest use the Unicode range.
pub(super) fn keysym_for_char(character: char) -> u32 {
    let code = u32::from(character);
    if (0x20..=0xff).contains(&code) {
        code
    } else {
        UNICODE_KEYSYM_BASE | code
    }
}

/// The keysym for a key egui names; modifier keys are handled by state diffing
/// and keys without a keysym (browser and clipboard keys) are dropped.
pub(super) fn keysym_for_key(pressed: Key) -> Option<u32> {
    let keysym = match pressed {
        Key::ArrowDown => XK_DOWN,
        Key::ArrowLeft => XK_LEFT,
        Key::ArrowRight => XK_RIGHT,
        Key::ArrowUp => XK_UP,
        Key::Escape => XK_ESCAPE,
        Key::Tab => XK_TAB,
        Key::Backspace => XK_BACKSPACE,
        Key::Enter => XK_RETURN,
        Key::Insert => XK_INSERT,
        Key::Delete => XK_DELETE,
        Key::Home => XK_HOME,
        Key::End => XK_END,
        Key::PageUp => XK_PAGE_UP,
        Key::PageDown => XK_PAGE_DOWN,
        Key::Space => u32::from(' '),
        Key::Minus => u32::from('-'),
        Key::Quote => u32::from('\''),
        Key::Copy
        | Key::Cut
        | Key::Paste
        | Key::BrowserBack
        | Key::ShiftLeft
        | Key::ShiftRight
        | Key::ControlLeft
        | Key::ControlRight
        | Key::AltLeft
        | Key::AltRight
        | Key::SuperLeft
        | Key::SuperRight
        | Key::IntlBackslash => return None,
        other => {
            let name = other.symbol_or_name();
            if let Some(number) = name.strip_prefix('F').and_then(|digits| digits.parse::<u32>().ok())
                && (1..=35).contains(&number)
                && name.len() <= 3
            {
                XK_F1 + number - 1
            } else {
                let mut characters = name.chars();
                match (characters.next(), characters.next()) {
                    (Some(character), None) if character.is_ascii() => keysym_for_char(character.to_ascii_lowercase()),
                    _ => return None,
                }
            }
        }
    };
    Some(keysym)
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::DeviceViewport;

    fn layout(x: usize, y: usize, width: usize, height: usize, output: [usize; 2]) -> DeviceImageLayout {
        DeviceImageLayout {
            source: DeviceViewport { x, y, width, height },
            output,
        }
    }

    fn keysyms(events: &[X11Event]) -> Vec<(u32, bool)> {
        events
            .iter()
            .filter_map(|event| match event {
                X11Event::KeyEvent(key) => Some((key.keycode, key.down)),
                _ => None,
            })
            .collect()
    }

    fn pointers(events: &[X11Event]) -> Vec<(u16, u16, u8)> {
        events
            .iter()
            .filter_map(|event| match event {
                X11Event::PointerEvent(mouse) => Some((mouse.position_x, mouse.position_y, mouse.bottons)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn image_points_map_through_the_crop_and_scale_to_desktop_pixels() {
        let image = Rect::from_min_size(Pos2::new(100.0, 50.0), egui::vec2(200.0, 100.0));
        let full = layout(0, 0, 1600, 800, [200, 100]);
        assert_eq!(
            desktop_point(image, &full, Pos2::new(100.0, 50.0)),
            Some(DesktopPoint { x: 0, y: 0 })
        );
        assert_eq!(
            desktop_point(image, &full, Pos2::new(200.0, 100.0)),
            Some(DesktopPoint { x: 800, y: 400 })
        );
        assert_eq!(
            desktop_point(image, &full, Pos2::new(299.9, 149.9)),
            Some(DesktopPoint { x: 1599, y: 799 }),
            "the far edge stays inside the desktop"
        );
        assert_eq!(
            desktop_point(image, &full, Pos2::new(99.0, 50.0)),
            None,
            "outside the image"
        );
        let cropped = layout(300, 200, 400, 200, [200, 100]);
        assert_eq!(
            desktop_point(image, &cropped, Pos2::new(150.0, 75.0)),
            Some(DesktopPoint { x: 400, y: 250 }),
            "a viewport offsets and scales the point"
        );
    }

    #[test]
    fn pointer_moves_and_button_changes_are_sent_once_each() {
        let mut state = InputState::default();
        let at = DesktopPoint { x: 10, y: 20 };
        assert_eq!(pointers(&state.pointer(Some(at), 0)), vec![(10, 20, 0)]);
        assert!(state.pointer(Some(at), 0).is_empty(), "no change, no event");
        assert_eq!(pointers(&state.pointer(Some(at), BUTTON_LEFT)), vec![(10, 20, 1)]);
        assert_eq!(
            pointers(&state.pointer(None, BUTTON_LEFT | BUTTON_RIGHT)),
            vec![(10, 20, 5)],
            "a drag that leaves the image keeps the last position"
        );
        assert!(
            InputState::default().pointer(None, BUTTON_LEFT).is_empty(),
            "nothing before a position"
        );
        assert_eq!(state.buttons(), 5);
        assert_eq!(button_bit(PointerButton::Middle), Some(2));
        assert_eq!(button_bit(PointerButton::Extra1), None);
    }

    #[test]
    fn scroll_travel_becomes_clicks_of_the_wheel_buttons() {
        let mut state = InputState::default();
        assert!(state.scroll(egui::vec2(0.0, 80.0)).is_empty(), "no position yet");
        state.pointer(Some(DesktopPoint { x: 1, y: 2 }), BUTTON_LEFT);
        assert_eq!(
            pointers(&state.scroll(egui::vec2(0.0, 80.0))),
            vec![(1, 2, 9), (1, 2, 1), (1, 2, 9), (1, 2, 1)],
            "two notches up keep the held button"
        );
        for _ in 0..3 {
            assert!(
                state.scroll(egui::vec2(-10.0, 0.0)).is_empty(),
                "small trackpad deltas accumulate"
            );
        }
        assert_eq!(
            pointers(&state.scroll(egui::vec2(-10.0, 0.0))),
            vec![(1, 2, 65), (1, 2, 1)],
            "four 10-point deltas make one notch, not four"
        );
        assert_eq!(
            state.scroll(egui::vec2(0.0, -100_000.0)).len(),
            2 * MAX_SCROLL_NOTCHES_PER_FRAME as usize
        );
        assert!(
            state.scroll(egui::Vec2::ZERO).is_empty(),
            "the excess beyond the cap is dropped"
        );
        assert!(state.scroll(egui::vec2(0.0, 20.0)).is_empty());
        state.release_all();
        assert!(
            state.scroll(egui::vec2(0.0, 20.0)).is_empty(),
            "a partial gesture does not carry across the end of capture"
        );
    }

    #[test]
    fn modifiers_are_diffed_and_released_with_everything_else() {
        let mut state = InputState::default();
        assert_eq!(
            keysyms(&state.modifiers(Modifiers::CTRL | Modifiers::SHIFT)),
            vec![(XK_SHIFT_L, true), (XK_CONTROL_L, true)]
        );
        assert!(state.modifiers(Modifiers::CTRL | Modifiers::SHIFT).is_empty());
        assert_eq!(keysyms(&state.modifiers(Modifiers::CTRL)), vec![(XK_SHIFT_L, false)]);
        assert_eq!(
            keysyms(&state.key(Key::C, true, Modifiers::CTRL)),
            vec![(u32::from('c'), true)]
        );
        assert_eq!(
            keysyms(&state.key(Key::C, false, Modifiers::NONE)),
            vec![(u32::from('c'), false)],
            "the release is sent even after Ctrl went up first"
        );
        assert!(state.key(Key::C, false, Modifiers::NONE).is_empty(), "and only once");
        assert_eq!(
            keysyms(&state.key(Key::C, true, Modifiers::CTRL)),
            vec![(u32::from('c'), true)]
        );
        state.pointer(Some(DesktopPoint { x: 3, y: 4 }), BUTTON_LEFT);
        let released = state.release_all();
        assert_eq!(
            keysyms(&released),
            vec![(u32::from('c'), false), (XK_CONTROL_L, false)],
            "held keys go up before the modifiers"
        );
        assert_eq!(pointers(&released), vec![(3, 4, 0)]);
        assert!(state.release_all().is_empty());
    }

    #[test]
    fn printable_keys_are_left_to_text_unless_ctrl_or_command_is_held() {
        let mut state = InputState::default();
        assert!(state.key(Key::A, true, Modifiers::NONE).is_empty());
        assert!(
            state.key(Key::A, true, Modifiers::SHIFT).is_empty(),
            "text carries the shifted character"
        );
        assert!(
            state.key(Key::A, true, Modifiers::ALT).is_empty(),
            "Alt still produces text on X11"
        );
        assert_eq!(
            keysyms(&state.key(Key::A, true, Modifiers::MAC_CMD)),
            vec![(u32::from('a'), true)]
        );
        assert_eq!(
            keysyms(&state.key(Key::Enter, true, Modifiers::NONE)),
            vec![(XK_RETURN, true)]
        );
        assert_eq!(
            keysyms(&state.key(Key::Enter, false, Modifiers::NONE)),
            vec![(XK_RETURN, false)]
        );
        assert!(
            state.key(Key::ShiftLeft, true, Modifiers::SHIFT).is_empty(),
            "modifiers come from the diff"
        );
        assert!(state.key(Key::Copy, true, Modifiers::NONE).is_empty());
        assert_eq!(
            keysyms(&InputState::text("aÉ€\n")),
            vec![
                (u32::from('a'), true),
                (u32::from('a'), false),
                (0xc9, true),
                (0xc9, false),
                (UNICODE_KEYSYM_BASE | 0x20ac, true),
                (UNICODE_KEYSYM_BASE | 0x20ac, false),
            ],
            "Latin-1 is direct, other scripts use the Unicode keysym range, controls are dropped"
        );
    }

    #[test]
    fn clipboard_chords_hold_the_command_modifier_and_swallow_their_release() {
        let control = if cfg!(target_os = "macos") {
            XK_SUPER_L
        } else {
            XK_CONTROL_L
        };
        let command = if cfg!(target_os = "macos") {
            Modifiers::MAC_CMD
        } else {
            Modifiers::CTRL
        };
        let (c, v) = (u32::from('c'), u32::from('v'));
        let mut state = InputState::default();
        // Ctrl already up by the end of the frame: the chord still holds it.
        assert_eq!(
            keysyms(&state.clipboard_chord('c')),
            vec![(control, true), (c, true), (c, false)]
        );
        assert!(
            state.key(Key::C, false, command).is_empty(),
            "the release was already sent"
        );
        assert_eq!(keysyms(&state.modifiers(Modifiers::NONE)), vec![(control, false)]);

        // Ctrl+V with an empty local clipboard: only the V release arrives,
        // and on X11 it comes after Ctrl's own release.
        assert_eq!(
            keysyms(&state.key(Key::V, false, Modifiers::NONE)),
            vec![(control, true), (v, true), (v, false)]
        );
        state.modifiers(Modifiers::NONE);
        // A typed v reports its press, so its release is not a paste.
        assert!(state.key(Key::V, true, Modifiers::NONE).is_empty());
        assert!(state.key(Key::V, false, Modifiers::NONE).is_empty());
        state.modifiers(command);
        // With a Paste event first, its release is swallowed instead.
        assert_eq!(keysyms(&state.clipboard_chord('v')), vec![(v, true), (v, false)]);
        assert!(state.key(Key::V, false, command).is_empty());
        assert!(state.key(Key::V, true, Modifiers::NONE).is_empty());
        state.clipboard_chord('x');
        state.release_all();
        assert_eq!(
            keysyms(&state.key(Key::V, false, Modifiers::NONE)),
            vec![(control, true), (v, true), (v, false)],
            "a plain V press before the end of capture does not hide the next paste"
        );
        state.release_all();
        assert!(
            state.key(Key::X, false, Modifiers::NONE).is_empty(),
            "a plain x release is text's business, and release_all dropped the pending chord"
        );
    }

    #[test]
    fn key_names_resolve_to_the_expected_keysyms() {
        assert_eq!(keysym_for_key(Key::F1), Some(XK_F1));
        assert_eq!(keysym_for_key(Key::F12), Some(XK_F1 + 11));
        assert_eq!(keysym_for_key(Key::F35), Some(XK_F1 + 34));
        assert_eq!(keysym_for_key(Key::Num7), Some(u32::from('7')));
        assert_eq!(keysym_for_key(Key::Z), Some(u32::from('z')));
        assert_eq!(keysym_for_key(Key::Colon), Some(u32::from(':')));
        assert_eq!(
            keysym_for_key(Key::Minus),
            Some(u32::from('-')),
            "not the typographic minus"
        );
        assert_eq!(keysym_for_key(Key::Quote), Some(u32::from('\'')));
        assert_eq!(keysym_for_key(Key::Space), Some(u32::from(' ')));
        assert_eq!(keysym_for_key(Key::Backtick), Some(u32::from('`')));
        assert_eq!(keysym_for_key(Key::Delete), Some(XK_DELETE));
        assert_eq!(keysym_for_key(Key::BrowserBack), None);
        assert_eq!(keysym_for_key(Key::IntlBackslash), None);
    }
}

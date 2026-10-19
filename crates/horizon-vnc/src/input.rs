//! A person's pointer and keyboard as VNC input. A UI maps its own events to
//! desktop points, keysyms and modifiers; [`InputState`] sends only what
//! changed and releases everything still held when capture ends. The rules
//! for clipboard shortcuts follow what egui-winit reports.
use std::ops::BitOr;

use crate::ImageLayout;

// X11 keysyms; VNC KeyEvent carries them verbatim.
pub const XK_BACKSPACE: u32 = 0xff08;
pub const XK_TAB: u32 = 0xff09;
pub const XK_RETURN: u32 = 0xff0d;
pub const XK_ESCAPE: u32 = 0xff1b;
pub const XK_HOME: u32 = 0xff50;
pub const XK_LEFT: u32 = 0xff51;
pub const XK_UP: u32 = 0xff52;
pub const XK_RIGHT: u32 = 0xff53;
pub const XK_DOWN: u32 = 0xff54;
pub const XK_PAGE_UP: u32 = 0xff55;
pub const XK_PAGE_DOWN: u32 = 0xff56;
pub const XK_END: u32 = 0xff57;
pub const XK_INSERT: u32 = 0xff63;
/// F1; F2 to F35 follow it.
pub const XK_F1: u32 = 0xffbe;
pub const XK_SHIFT_L: u32 = 0xffe1;
pub const XK_CONTROL_L: u32 = 0xffe3;
pub const XK_ALT_L: u32 = 0xffe9;
pub const XK_SUPER_L: u32 = 0xffeb;
pub const XK_DELETE: u32 = 0xffff;
/// Keysyms above Latin-1 are the Unicode code point with this bit set.
pub const UNICODE_KEYSYM_BASE: u32 = 0x0100_0000;

/// RFB pointer button mask bits.
pub const BUTTON_LEFT: u8 = 1;
pub const BUTTON_MIDDLE: u8 = 2;
pub const BUTTON_RIGHT: u8 = 4;
const BUTTON_SCROLL_UP: u8 = 8;
const BUTTON_SCROLL_DOWN: u8 = 16;
const BUTTON_SCROLL_LEFT: u8 = 32;
const BUTTON_SCROLL_RIGHT: u8 = 64;
/// One scroll notch per this many logical points of wheel travel.
pub const SCROLL_NOTCH_POINTS: f32 = 40.0;
const MAX_SCROLL_NOTCHES_PER_FRAME: u32 = 8;

/// One input message for the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEvent {
    /// An X11 keysym goes down or up.
    Key { keysym: u32, down: bool },
    /// The pointer is at a desktop pixel with these RFB buttons held.
    Pointer { x: u16, y: u16, buttons: u8 },
}

/// The modifier keys that are down. `command` is the macOS Command key; it is
/// sent as `Super_L`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub command: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
        command: false,
    };
    pub const SHIFT: Self = Self {
        shift: true,
        ..Self::NONE
    };
    pub const CTRL: Self = Self {
        ctrl: true,
        ..Self::NONE
    };
    pub const ALT: Self = Self {
        alt: true,
        ..Self::NONE
    };
    pub const COMMAND: Self = Self {
        command: true,
        ..Self::NONE
    };
}

impl BitOr for Modifiers {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self {
            shift: self.shift || other.shift,
            ctrl: self.ctrl || other.ctrl,
            alt: self.alt || other.alt,
            command: self.command || other.command,
        }
    }
}

/// Where the pointer is on the desktop, in server pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesktopPoint {
    pub x: u16,
    pub y: u16,
}

/// Where a UI draws the image, in its own coordinates (for example egui
/// points), from the top-left corner `min` to the bottom-right corner `max`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenRect {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

/// Map a point inside the rendered image back to the desktop pixel it shows,
/// honouring the active crop and scaling. Outside the image is `None`; the
/// edges are inside.
#[must_use]
pub fn desktop_point(image: ScreenRect, layout: &ImageLayout, pos: [f32; 2]) -> Option<DesktopPoint> {
    let [width, height] = [image.max[0] - image.min[0], image.max[1] - image.min[1]];
    let inside = (image.min[0]..=image.max[0]).contains(&pos[0]) && (image.min[1]..=image.max[1]).contains(&pos[1]);
    if !inside || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let source = layout.source;
    let relative_x = ((pos[0] - image.min[0]) / width).clamp(0.0, 1.0);
    let relative_y = ((pos[1] - image.min[1]) / height).clamp(0.0, 1.0);
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
pub struct InputState {
    buttons: u8,
    position: Option<DesktopPoint>,
    modifiers: Modifiers,
    held_keys: Vec<u32>,
    /// Wheel travel not yet worth a whole notch; trackpads deliver many small
    /// deltas that must add up rather than each become a click.
    scroll_remainder: [f32; 2],
    /// Clipboard chords already sent whose key release is still to come.
    pending_clipboard: Vec<char>,
    /// egui-winit reported a V press since the last V release. It reports one
    /// for every V except command+V, so a release without one was a paste.
    v_pressed: bool,
}

impl InputState {
    /// The button mask the desktop currently believes is held.
    #[must_use]
    pub fn buttons(&self) -> u8 {
        self.buttons
    }

    /// Pointer events for this frame: a move when the position changed and a
    /// button event when the mask changed. `position` is `None` off the image.
    pub fn pointer(&mut self, position: Option<DesktopPoint>, buttons: u8) -> Vec<InputEvent> {
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

    /// Wheel travel in points (`[x, y]`, positive up and left) as scroll
    /// button clicks at the current pointer position. Travel is accumulated
    /// across frames and only whole notches are sent; the part beyond the
    /// per-frame cap is dropped rather than queued.
    pub fn scroll(&mut self, delta: [f32; 2]) -> Vec<InputEvent> {
        let Some(position) = self.position else {
            return Vec::new();
        };
        self.scroll_remainder[0] += delta[0];
        self.scroll_remainder[1] += delta[1];
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
    pub fn modifiers(&mut self, modifiers: Modifiers) -> Vec<InputEvent> {
        let mut events = Vec::new();
        for (was, now, keysym) in [
            (self.modifiers.shift, modifiers.shift, XK_SHIFT_L),
            (self.modifiers.ctrl, modifiers.ctrl, XK_CONTROL_L),
            (self.modifiers.alt, modifiers.alt, XK_ALT_L),
            (self.modifiers.command, modifiers.command, XK_SUPER_L),
        ] {
            if was != now {
                events.push(key(keysym, now));
            }
        }
        self.modifiers = modifiers;
        events
    }

    /// A key press or release, as its keysym. Letters are lowercase, as
    /// [`keysym_for_char`] gives them. Printable keys without Ctrl or Command
    /// arrive again as text, so they are left to [`Self::text`]; everything
    /// else is sent as its keysym. Modifier keys come from [`Self::modifiers`].
    pub fn key(&mut self, keysym: u32, down: bool, modifiers: Modifiers) -> Vec<InputEvent> {
        if keysym == u32::from('v') && down {
            self.v_pressed = true;
        }
        if !down && let Some(events) = self.clipboard_release(keysym) {
            return events;
        }
        let printable = keysym < 0xff00;
        // A release always follows its press, even if the modifier that made
        // the press a chord went up first; otherwise the key stays held.
        let releasing_held = !down && self.held_keys.contains(&keysym);
        if printable && !(modifiers.ctrl || modifiers.command) && !releasing_held {
            return Vec::new();
        }
        self.track(keysym, down);
        vec![key(keysym, down)]
    }

    /// A clipboard shortcut egui-winit reported as Copy, Cut or Paste: the
    /// letter with the platform command modifier held around it, whatever
    /// the modifier state was by the end of the frame. The key's release,
    /// which egui-winit still reports, is then swallowed.
    pub fn clipboard_chord(&mut self, letter: char) -> Vec<InputEvent> {
        let mut chord = self.modifiers;
        if cfg!(target_os = "macos") {
            chord.command = true;
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
    #[must_use]
    pub fn tap(letter: char) -> Vec<InputEvent> {
        let keysym = keysym_for_char(letter);
        vec![key(keysym, true), key(keysym, false)]
    }

    /// The release of C, X or V after a clipboard chord is swallowed. A V
    /// release with neither a V press nor a Paste before it was command+V
    /// with an empty local clipboard, which egui-winit reports as nothing but
    /// that release (often after the modifier's own release); the chord is
    /// sent now.
    fn clipboard_release(&mut self, keysym: u32) -> Option<Vec<InputEvent>> {
        let letter = ['c', 'x', 'v']
            .into_iter()
            .find(|letter| u32::from(*letter) == keysym)?;
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
    #[must_use]
    pub fn text(text: &str) -> Vec<InputEvent> {
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
    pub fn release_all(&mut self) -> Vec<InputEvent> {
        // A partial wheel gesture must not carry into the next capture.
        self.scroll_remainder = [0.0; 2];
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

const fn key(keysym: u32, down: bool) -> InputEvent {
    InputEvent::Key { keysym, down }
}

const fn mouse(position: DesktopPoint, buttons: u8) -> InputEvent {
    InputEvent::Pointer {
        x: position.x,
        y: position.y,
        buttons,
    }
}

/// Latin-1 characters are their own keysym; the rest use the Unicode range.
#[must_use]
pub fn keysym_for_char(character: char) -> u32 {
    let code = u32::from(character);
    if (0x20..=0xff).contains(&code) {
        code
    } else {
        UNICODE_KEYSYM_BASE | code
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Region;

    fn layout(x: usize, y: usize, width: usize, height: usize, output: [usize; 2]) -> ImageLayout {
        ImageLayout {
            source: Region { x, y, width, height },
            output,
        }
    }

    fn keysyms(events: &[InputEvent]) -> Vec<(u32, bool)> {
        events
            .iter()
            .filter_map(|event| match event {
                InputEvent::Key { keysym, down } => Some((*keysym, *down)),
                InputEvent::Pointer { .. } => None,
            })
            .collect()
    }

    fn pointers(events: &[InputEvent]) -> Vec<(u16, u16, u8)> {
        events
            .iter()
            .filter_map(|event| match event {
                InputEvent::Pointer { x, y, buttons } => Some((*x, *y, *buttons)),
                InputEvent::Key { .. } => None,
            })
            .collect()
    }

    const C: u32 = 0x63;
    const V: u32 = 0x76;
    const X: u32 = 0x78;

    #[test]
    fn image_points_map_through_the_crop_and_scale_to_desktop_pixels() {
        let image = ScreenRect {
            min: [100.0, 50.0],
            max: [300.0, 150.0],
        };
        let full = layout(0, 0, 1600, 800, [200, 100]);
        assert_eq!(
            desktop_point(image, &full, [100.0, 50.0]),
            Some(DesktopPoint { x: 0, y: 0 })
        );
        assert_eq!(
            desktop_point(image, &full, [200.0, 100.0]),
            Some(DesktopPoint { x: 800, y: 400 })
        );
        assert_eq!(
            desktop_point(image, &full, [299.9, 149.9]),
            Some(DesktopPoint { x: 1599, y: 799 }),
            "the far edge stays inside the desktop"
        );
        assert_eq!(desktop_point(image, &full, [99.0, 50.0]), None, "outside the image");
        let cropped = layout(300, 200, 400, 200, [200, 100]);
        assert_eq!(
            desktop_point(image, &cropped, [150.0, 75.0]),
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
    }

    #[test]
    fn scroll_travel_becomes_clicks_of_the_wheel_buttons() {
        let mut state = InputState::default();
        assert!(state.scroll([0.0, 80.0]).is_empty(), "no position yet");
        state.pointer(Some(DesktopPoint { x: 1, y: 2 }), BUTTON_LEFT);
        assert_eq!(
            pointers(&state.scroll([0.0, 80.0])),
            vec![(1, 2, 9), (1, 2, 1), (1, 2, 9), (1, 2, 1)],
            "two notches up keep the held button"
        );
        for _ in 0..3 {
            assert!(
                state.scroll([-10.0, 0.0]).is_empty(),
                "small trackpad deltas accumulate"
            );
        }
        assert_eq!(
            pointers(&state.scroll([-10.0, 0.0])),
            vec![(1, 2, 65), (1, 2, 1)],
            "four 10-point deltas make one notch, not four"
        );
        assert_eq!(
            state.scroll([0.0, -100_000.0]).len(),
            2 * MAX_SCROLL_NOTCHES_PER_FRAME as usize
        );
        assert!(
            state.scroll([0.0, 0.0]).is_empty(),
            "the excess beyond the cap is dropped"
        );
        assert!(state.scroll([0.0, 20.0]).is_empty());
        state.release_all();
        assert!(
            state.scroll([0.0, 20.0]).is_empty(),
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
            keysyms(&state.key(C, true, Modifiers::CTRL)),
            vec![(u32::from('c'), true)]
        );
        assert_eq!(
            keysyms(&state.key(C, false, Modifiers::NONE)),
            vec![(u32::from('c'), false)],
            "the release is sent even after Ctrl went up first"
        );
        assert!(state.key(C, false, Modifiers::NONE).is_empty(), "and only once");
        assert_eq!(
            keysyms(&state.key(C, true, Modifiers::CTRL)),
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
        let a = u32::from('a');
        assert!(state.key(a, true, Modifiers::NONE).is_empty());
        assert!(
            state.key(a, true, Modifiers::SHIFT).is_empty(),
            "text carries the shifted character"
        );
        assert!(
            state.key(a, true, Modifiers::ALT).is_empty(),
            "Alt still produces text on X11"
        );
        assert_eq!(keysyms(&state.key(a, true, Modifiers::COMMAND)), vec![(a, true)]);
        assert_eq!(
            keysyms(&state.key(XK_RETURN, true, Modifiers::NONE)),
            vec![(XK_RETURN, true)]
        );
        assert_eq!(
            keysyms(&state.key(XK_RETURN, false, Modifiers::NONE)),
            vec![(XK_RETURN, false)]
        );
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
            Modifiers::COMMAND
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
        assert!(state.key(C, false, command).is_empty(), "the release was already sent");
        assert_eq!(keysyms(&state.modifiers(Modifiers::NONE)), vec![(control, false)]);

        // Ctrl+V with an empty local clipboard: only the V release arrives,
        // and on X11 it comes after Ctrl's own release.
        assert_eq!(
            keysyms(&state.key(V, false, Modifiers::NONE)),
            vec![(control, true), (v, true), (v, false)]
        );
        state.modifiers(Modifiers::NONE);
        // A typed v reports its press, so its release is not a paste.
        assert!(state.key(V, true, Modifiers::NONE).is_empty());
        assert!(state.key(V, false, Modifiers::NONE).is_empty());
        state.modifiers(command);
        // With a Paste event first, its release is swallowed instead.
        assert_eq!(keysyms(&state.clipboard_chord('v')), vec![(v, true), (v, false)]);
        assert!(state.key(V, false, command).is_empty());
        assert!(state.key(V, true, Modifiers::NONE).is_empty());
        state.clipboard_chord('x');
        state.release_all();
        assert_eq!(
            keysyms(&state.key(V, false, Modifiers::NONE)),
            vec![(control, true), (v, true), (v, false)],
            "a plain V press before the end of capture does not hide the next paste"
        );
        state.release_all();
        assert!(
            state.key(X, false, Modifiers::NONE).is_empty(),
            "a plain x release is text's business, and release_all dropped the pending chord"
        );
    }
}

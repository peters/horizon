//! egui → CDP input translation for the focused browser panel.

use egui::{Event, Key, Modifiers, PointerButton, Ui};
use horizon_core::browser::{
    BrowserButton, BrowserCommand, BrowserInput, BrowserKey, BrowserModifiers, BrowserPanelState,
};

use crate::browser_widget::BrowserUiState;

/// CDP `buttons` bitmask for currently-down mouse buttons.
const BUTTON_LEFT: u32 = 1;
const BUTTON_MIDDLE: u32 = 2;
const BUTTON_RIGHT: u32 = 4;

/// Focus/interaction flags for one frame.
#[derive(Clone, Copy)]
pub(crate) struct InputFlags {
    pub(crate) is_focused: bool,
    pub(crate) interactive: bool,
    pub(crate) url_focused: bool,
}

pub fn handle(
    ui: &mut Ui,
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    body: Option<egui::Rect>,
    frame_size: Option<[f32; 2]>,
    flags: InputFlags,
) {
    if !flags.interactive {
        return;
    }
    if let (Some(rect), Some(frame_size)) = (body, frame_size) {
        pointer_events(ui, browser, state, rect, frame_size);
    }
    if flags.is_focused && !flags.url_focused {
        keyboard_events(ui, browser);
    }
}

fn pointer_events(
    ui: &Ui,
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    rect: egui::Rect,
    frame_size: [f32; 2],
) {
    let ctx = ui.ctx();
    // Pointer positions arrive in global (window) space; the body rect is
    // in this layer's local space (panels are drawn on a transformed
    // canvas), so transform before hit-testing — same pattern as the
    // terminal widget's input.
    let from_global = ctx.layer_transform_from_global(ui.layer_id());
    let transform = |pos: egui::Pos2| from_global.map_or(pos, |t| t * pos);
    let (buttons, frame_pos) = ctx.input(|i| {
        let buttons = (if i.pointer.primary_down() { BUTTON_LEFT } else { 0 })
            | (if i.pointer.middle_down() { BUTTON_MIDDLE } else { 0 })
            | (if i.pointer.secondary_down() { BUTTON_RIGHT } else { 0 });
        (buttons, i.pointer.interact_pos())
    });
    let modifiers = key_modifiers(ui);
    // Frame-end pointer position: only used for wheel events, which carry
    // no per-event position.
    let wheel_pos = frame_pos.map(transform).filter(|p| rect.contains(*p));

    // Replay this frame's pointer events with their own positions. A fast
    // gesture (press, moves, release) can be batched into a single
    // rendered frame; sampling only the frame-end position would collapse
    // the whole drag to one point. A panel only owns a press that started
    // inside its body, so a drag released over another panel (or the
    // canvas) still ends with exactly one release. Chrome coalesces
    // mouseMoved messages that arrive in one batch to the last position,
    // so only the final move of the frame is forwarded.
    let mut pending_move: Option<(f64, f64)> = None;
    for event in ctx.input(|i| i.events.clone()) {
        match event {
            Event::PointerButton {
                pos, button, pressed, ..
            } => {
                let Some(browser_button) = egui_button(button) else {
                    continue;
                };
                let p = transform(pos);
                let (x, y) = to_page_coords(rect, frame_size, p);
                if pressed {
                    if rect.contains(p) {
                        state.captured_button = Some(browser_button);
                        state.last_mouse = Some(p);
                        browser.send(BrowserCommand::Input(BrowserInput::MousePress {
                            x,
                            y,
                            button: browser_button,
                            click_count: 1,
                            buttons: buttons | button_mask(browser_button),
                            modifiers,
                        }));
                    }
                } else if state.captured_button == Some(browser_button) {
                    state.captured_button = None;
                    state.last_mouse = Some(p);
                    // A release supersedes any buffered move at the same
                    // spot: drop it to keep the CDP stream minimal.
                    pending_move = None;
                    browser.send(BrowserCommand::Input(BrowserInput::MouseRelease {
                        x,
                        y,
                        button: browser_button,
                        click_count: 1,
                        buttons,
                        modifiers,
                    }));
                }
            }
            Event::PointerMoved(pos) => {
                let p = transform(pos);
                let tracking = rect.contains(p) || state.captured_button.is_some();
                // Movement dedup: only forward real movement. `None` (the
                // first move, or the first after PointerGone) must be
                // forwarded, or hover would be dead until a click.
                if tracking && state.last_mouse.is_none_or(|last| (last - p).length() >= 0.5) {
                    let (x, y) = to_page_coords(rect, frame_size, p);
                    state.last_mouse = Some(p);
                    pending_move = Some((x, y));
                }
            }
            // The pointer left the window mid-drag: end the drag where we
            // last saw it instead of stranding Chrome's button state down.
            Event::PointerGone => {
                if let Some(button) = state.captured_button.take()
                    && let Some(p) = state.last_mouse
                {
                    let (x, y) = to_page_coords(rect, frame_size, p);
                    pending_move = None;
                    browser.send(BrowserCommand::Input(BrowserInput::MouseRelease {
                        x,
                        y,
                        button,
                        click_count: 1,
                        buttons: 0,
                        modifiers,
                    }));
                    state.last_mouse = None;
                }
            }
            Event::MouseWheel { unit, delta, .. } => {
                if let Some(p) = wheel_pos {
                    let scale = match unit {
                        egui::MouseWheelUnit::Point => 1.0,
                        egui::MouseWheelUnit::Line => 16.0,
                        egui::MouseWheelUnit::Page => 500.0,
                    };
                    let (x, y) = to_page_coords(rect, frame_size, p);
                    browser.send(BrowserCommand::Input(BrowserInput::Wheel {
                        x,
                        y,
                        delta_x: f64::from(delta.x * scale),
                        delta_y: f64::from(delta.y * scale),
                        modifiers,
                    }));
                }
            }
            _ => {}
        }
    }
    if let Some((x, y)) = pending_move {
        browser.send(BrowserCommand::Input(BrowserInput::MouseMove { x, y, buttons }));
    }
}

/// egui mouse button → CDP button (mouse-only; touch is not forwarded).
fn egui_button(button: PointerButton) -> Option<BrowserButton> {
    match button {
        PointerButton::Primary => Some(BrowserButton::Left),
        PointerButton::Middle => Some(BrowserButton::Middle),
        PointerButton::Secondary => Some(BrowserButton::Right),
        _ => None,
    }
}

fn keyboard_events(ui: &Ui, browser: &mut BrowserPanelState) {
    let ctx = ui.ctx();
    let events = ctx.input(|i| i.events.clone());
    // Characters we deliver via the Key event's `text` (CDP `char`);
    // egui also delivers them as `Event::Text`, so those are deduped below.
    // The set must mirror exactly what the Key arm sends — including the
    // shift adjustment — or Shift+letter would type twice.
    let mut key_chars: Vec<char> = Vec::new();
    for event in &events {
        if let Event::Key {
            key,
            pressed,
            repeat,
            modifiers,
            ..
        } = event
            && *pressed
            && !*repeat
            && !(modifiers.ctrl || modifiers.command || modifiers.alt)
            && let Some(c) = key_to_browser_key(*key).and_then(BrowserKey::printable_char)
        {
            key_chars.push(if modifiers.shift { shift_char(c) } else { c });
        }
    }
    for event in events {
        match event {
            Event::Text(text) if !text.is_empty() => {
                let duplicate =
                    text.chars().count() == 1 && text.chars().next().is_some_and(|c| key_chars.contains(&c));
                if !duplicate {
                    browser.send(BrowserCommand::Input(BrowserInput::InsertText { text }));
                }
            }
            // egui_winit turns Ctrl/Cmd+V into a global Paste event (the
            // URL bar consumes its own copy while focused; the page gets
            // the text via CDP insertText, matching a native paste).
            Event::Paste(text) if !text.is_empty() => {
                browser.send(BrowserCommand::Input(BrowserInput::InsertText { text }));
            }
            Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } => {
                if !pressed {
                    // The press side swallowed the reload shortcuts (F5,
                    // Ctrl/Cmd+R) without forwarding a keydown; drop the
                    // orphan keyup so the page never sees a release with no
                    // press. Plain R and plain F5 have no other meaning,
                    // so nothing page-visible is lost.
                    if key == Key::F5 || (key == Key::R && (modifiers.ctrl || modifiers.command)) {
                        continue;
                    }
                    if let Some(browser_key) = key_to_browser_key(key) {
                        browser.send(BrowserCommand::Input(BrowserInput::KeyUp {
                            key: browser_key,
                            modifiers: to_browser_modifiers(modifiers),
                        }));
                    }
                    continue;
                }
                // Panel-level shortcuts (first press only; held keys keep
                // going to the page).
                if !repeat && (key == Key::F5 || (key == Key::R && (modifiers.ctrl || modifiers.command))) {
                    browser.send(horizon_core::browser::BrowserCommand::Reload);
                    continue;
                }
                let Some(browser_key) = key_to_browser_key(key) else {
                    continue;
                };
                let modifiers = to_browser_modifiers(modifiers);
                // Shortcut chords (Ctrl/Cmd/Alt + key) go out as raw key
                // events so the page sees them; plain or Shift chords
                // deliver the (shift-adjusted) character. Held-key repeats
                // are forwarded with `autoRepeat` so e.g. Backspace works.
                let text = browser_key
                    .printable_char()
                    .map(|c| if modifiers.shift { shift_char(c) } else { c })
                    .map(|c| c.to_string())
                    .filter(|_| !(modifiers.ctrl || modifiers.meta || modifiers.alt));
                browser.send(BrowserCommand::Input(BrowserInput::KeyDown {
                    key: browser_key,
                    text,
                    modifiers,
                    repeat,
                }));
            }
            _ => {}
        }
    }
}

/// Shift-adjusted character for the US layout (letters + the symbols the
/// key map covers).
fn shift_char(c: char) -> char {
    match c {
        'a'..='z' => c.to_ascii_uppercase(),
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        '`' => '~',
        other => other,
    }
}

fn to_browser_modifiers(modifiers: Modifiers) -> BrowserModifiers {
    BrowserModifiers {
        alt: modifiers.alt,
        ctrl: modifiers.ctrl,
        meta: modifiers.command,
        shift: modifiers.shift,
    }
}

fn key_modifiers(ui: &Ui) -> BrowserModifiers {
    ui.input(|i| to_browser_modifiers(i.modifiers))
}

fn to_page_coords(rect: egui::Rect, frame_size: [f32; 2], pos: egui::Pos2) -> (f64, f64) {
    let x = f64::from(((pos.x - rect.min.x) / rect.width() * frame_size[0]).clamp(0.0, frame_size[0]));
    let y = f64::from(((pos.y - rect.min.y) / rect.height() * frame_size[1]).clamp(0.0, frame_size[1]));
    (x, y)
}

fn button_mask(button: BrowserButton) -> u32 {
    match button {
        BrowserButton::Left => BUTTON_LEFT,
        BrowserButton::Middle => BUTTON_MIDDLE,
        BrowserButton::Right => BUTTON_RIGHT,
    }
}

fn key_to_browser_key(key: Key) -> Option<BrowserKey> {
    Some(match key {
        Key::ArrowUp => BrowserKey::ArrowUp,
        Key::ArrowDown => BrowserKey::ArrowDown,
        Key::ArrowLeft => BrowserKey::ArrowLeft,
        Key::ArrowRight => BrowserKey::ArrowRight,
        Key::Enter => BrowserKey::Enter,
        Key::Tab => BrowserKey::Tab,
        Key::Escape => BrowserKey::Escape,
        Key::Space => BrowserKey::Space,
        Key::Backspace => BrowserKey::Backspace,
        Key::Delete => BrowserKey::Delete,
        Key::Home => BrowserKey::Home,
        Key::End => BrowserKey::End,
        Key::PageUp => BrowserKey::PageUp,
        Key::PageDown => BrowserKey::PageDown,
        Key::Insert => BrowserKey::Insert,
        Key::F1 => BrowserKey::F1,
        Key::F2 => BrowserKey::F2,
        Key::F3 => BrowserKey::F3,
        Key::F4 => BrowserKey::F4,
        Key::F5 => BrowserKey::F5,
        Key::F6 => BrowserKey::F6,
        Key::F7 => BrowserKey::F7,
        Key::F8 => BrowserKey::F8,
        Key::F9 => BrowserKey::F9,
        Key::F10 => BrowserKey::F10,
        Key::F11 => BrowserKey::F11,
        Key::F12 => BrowserKey::F12,
        Key::A => BrowserKey::Char('a'),
        Key::B => BrowserKey::Char('b'),
        Key::C => BrowserKey::Char('c'),
        Key::D => BrowserKey::Char('d'),
        Key::E => BrowserKey::Char('e'),
        Key::F => BrowserKey::Char('f'),
        Key::G => BrowserKey::Char('g'),
        Key::H => BrowserKey::Char('h'),
        Key::I => BrowserKey::Char('i'),
        Key::J => BrowserKey::Char('j'),
        Key::K => BrowserKey::Char('k'),
        Key::L => BrowserKey::Char('l'),
        Key::M => BrowserKey::Char('m'),
        Key::N => BrowserKey::Char('n'),
        Key::O => BrowserKey::Char('o'),
        Key::P => BrowserKey::Char('p'),
        Key::Q => BrowserKey::Char('q'),
        Key::R => BrowserKey::Char('r'),
        Key::S => BrowserKey::Char('s'),
        Key::T => BrowserKey::Char('t'),
        Key::U => BrowserKey::Char('u'),
        Key::V => BrowserKey::Char('v'),
        Key::W => BrowserKey::Char('w'),
        Key::X => BrowserKey::Char('x'),
        Key::Y => BrowserKey::Char('y'),
        Key::Z => BrowserKey::Char('z'),
        Key::Num0 => BrowserKey::Char('0'),
        Key::Num1 => BrowserKey::Char('1'),
        Key::Num2 => BrowserKey::Char('2'),
        Key::Num3 => BrowserKey::Char('3'),
        Key::Num4 => BrowserKey::Char('4'),
        Key::Num5 => BrowserKey::Char('5'),
        Key::Num6 => BrowserKey::Char('6'),
        Key::Num7 => BrowserKey::Char('7'),
        Key::Num8 => BrowserKey::Char('8'),
        Key::Num9 => BrowserKey::Char('9'),
        Key::Comma => BrowserKey::Char(','),
        Key::Period => BrowserKey::Char('.'),
        Key::Slash => BrowserKey::Char('/'),
        Key::Semicolon => BrowserKey::Char(';'),
        Key::Quote => BrowserKey::Char('\''),
        Key::Minus => BrowserKey::Char('-'),
        Key::Equals => BrowserKey::Char('='),
        Key::OpenBracket => BrowserKey::Char('['),
        Key::CloseBracket => BrowserKey::Char(']'),
        Key::Backslash => BrowserKey::Char('\\'),
        Key::Backtick => BrowserKey::Char('`'),
        _ => return None,
    })
}

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
    let (pos, down_left, down_middle, down_right, pressed_l, pressed_m, pressed_r, released_l, released_m, released_r) =
        ctx.input(|i| {
            (
                i.pointer.interact_pos(),
                i.pointer.primary_down(),
                i.pointer.middle_down(),
                i.pointer.secondary_down(),
                i.pointer.primary_pressed(),
                i.pointer.button_pressed(PointerButton::Middle),
                i.pointer.secondary_pressed(),
                i.pointer.button_released(PointerButton::Primary),
                i.pointer.button_released(PointerButton::Middle),
                i.pointer.button_released(PointerButton::Secondary),
            )
        });
    let Some(pos) = pos else {
        return;
    };
    let buttons = (if down_left { BUTTON_LEFT } else { 0 })
        | (if down_middle { BUTTON_MIDDLE } else { 0 })
        | (if down_right { BUTTON_RIGHT } else { 0 });
    let modifiers = key_modifiers(ui);
    let inside = rect.contains(pos);
    // While a button is held, keep tracking outside the panel (drags).
    let tracking = inside || buttons != 0;
    if !tracking {
        state.last_mouse = None;
        return;
    }

    let (page_x, page_y) = to_page_coords(rect, frame_size, pos);

    if pressed_l || pressed_m || pressed_r {
        let button = if pressed_l {
            BrowserButton::Left
        } else if pressed_m {
            BrowserButton::Middle
        } else {
            BrowserButton::Right
        };
        browser.send(BrowserCommand::Input(BrowserInput::MousePress {
            x: page_x,
            y: page_y,
            button,
            click_count: 1,
            buttons: buttons | button_mask(button),
            modifiers,
        }));
        state.last_mouse = Some(pos);
        return;
    }
    if released_l || released_m || released_r {
        let button = if released_l {
            BrowserButton::Left
        } else if released_m {
            BrowserButton::Middle
        } else {
            BrowserButton::Right
        };
        browser.send(BrowserCommand::Input(BrowserInput::MouseRelease {
            x: page_x,
            y: page_y,
            button,
            click_count: 1,
            buttons,
            modifiers,
        }));
        state.last_mouse = Some(pos);
        return;
    }
    // Movement dedup: only forward real movement.
    if let Some(last) = state.last_mouse
        && (last - pos).length() < 0.5
    {
        return;
    }
    state.last_mouse = Some(pos);
    browser.send(BrowserCommand::Input(BrowserInput::MouseMove {
        x: page_x,
        y: page_y,
        buttons,
    }));

    // Wheel: egui delivers MouseWheel events in the frame's event list.
    let wheel_events = ctx.input(|i| i.events.clone());
    for event in wheel_events {
        if let Event::MouseWheel { unit, delta, .. } = event
            && inside
        {
            let scale = match unit {
                egui::MouseWheelUnit::Point => 1.0,
                egui::MouseWheelUnit::Line => 16.0,
                egui::MouseWheelUnit::Page => 500.0,
            };
            browser.send(BrowserCommand::Input(BrowserInput::Wheel {
                x: page_x,
                y: page_y,
                delta_x: f64::from(delta.x * scale),
                delta_y: f64::from(delta.y * scale),
                modifiers,
            }));
        }
    }
}

fn keyboard_events(ui: &Ui, browser: &mut BrowserPanelState) {
    let ctx = ui.ctx();
    let events = ctx.input(|i| i.events.clone());
    for event in events {
        match event {
            Event::Text(text) if !text.is_empty() => {
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
                    if let Some(browser_key) = key_to_browser_key(key) {
                        browser.send(BrowserCommand::Input(BrowserInput::KeyUp {
                            key: browser_key,
                            modifiers: to_browser_modifiers(modifiers),
                        }));
                    }
                    continue;
                }
                if repeat {
                    continue;
                }
                // Panel-level shortcuts.
                if key == Key::F5 || (key == Key::R && (modifiers.ctrl || modifiers.command)) {
                    browser.send(horizon_core::browser::BrowserCommand::Reload);
                    continue;
                }
                let Some(browser_key) = key_to_browser_key(key) else {
                    continue;
                };
                browser.send(BrowserCommand::Input(BrowserInput::KeyDown {
                    key: browser_key,
                    text: browser_key.printable_char().map(|c| c.to_string()),
                    modifiers: to_browser_modifiers(modifiers),
                }));
            }
            _ => {}
        }
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

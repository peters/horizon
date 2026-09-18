use alacritty_terminal::term::TermMode;
use egui::{Context, Id, Modifiers, MouseWheelUnit, PointerButton, Vec2};

use super::keyboard::control_modifier;
use super::{GridPoint, PointerButtons};

pub enum WheelAction {
    Pty(Vec<u8>),
    Scrollback(i32),
}

pub fn mouse_button_report(
    button: PointerButton,
    pressed: bool,
    modifiers: Modifiers,
    mode: TermMode,
    point: GridPoint,
) -> Option<Vec<u8>> {
    if modifiers.shift || !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }

    let code = match button {
        PointerButton::Primary => 0,
        PointerButton::Middle => 1,
        PointerButton::Secondary => 2,
        PointerButton::Extra1 | PointerButton::Extra2 => return None,
    };

    Some(mouse_report(code, pressed, modifiers, mode, point))
}

pub fn mouse_motion_report(
    buttons: PointerButtons,
    modifiers: Modifiers,
    mode: TermMode,
    point: GridPoint,
) -> Option<Vec<u8>> {
    if modifiers.shift || !mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG) {
        return None;
    }

    let code = if buttons.primary {
        Some(32)
    } else if buttons.middle {
        Some(33)
    } else if buttons.secondary {
        Some(34)
    } else if mode.contains(TermMode::MOUSE_MOTION) {
        Some(35)
    } else {
        None
    }?;

    Some(mouse_report(code, true, modifiers, mode, point))
}

/// Unmodified two-finger/wheel motion pans the canvas even over a panel.
/// Shift+wheel, or wheel during a primary-button gesture (text selection),
/// still belongs to the panel body.
///
/// This is the board-view rule, where the canvas pan handler is running.
/// Fullscreen (and any other path that skips that handler) must pass
/// `canvas_claims = false` so unmodified wheel stays on the panel.
#[must_use]
pub fn panel_content_owns_wheel(modifiers: Modifiers, primary_down: bool) -> bool {
    panel_content_owns_wheel_with_canvas_claim(modifiers, primary_down, true)
}

/// Same ownership rule as [`panel_content_owns_wheel`], but unmodified wheel
/// stays on the panel when the canvas pan handler is not claiming it.
#[must_use]
pub fn panel_content_owns_wheel_with_canvas_claim(
    modifiers: Modifiers,
    primary_down: bool,
    canvas_claims: bool,
) -> bool {
    if modifiers.ctrl || modifiers.command {
        return false;
    }
    modifiers.shift || primary_down || !canvas_claims
}

fn canvas_claims_unmodified_wheel_id(ctx: &Context) -> Id {
    Id::new(("horizon_canvas_claims_unmodified_wheel", ctx.viewport_id()))
}

/// Whether this viewport's canvas pan handler is claiming unmodified wheel.
///
/// Unset means the handler has not run; widget tests treat that as the
/// board-view default (canvas claims). The app clears the flag at the start
/// of each root frame so fullscreen, which skips the handler, defaults to
/// panel scroll.
#[must_use]
pub fn canvas_claims_unmodified_wheel(ctx: &Context) -> bool {
    let id = canvas_claims_unmodified_wheel_id(ctx);
    ctx.data(|data| data.get_temp::<bool>(id)).unwrap_or(true)
}

pub fn set_canvas_claims_unmodified_wheel(ctx: &Context, claims: bool) {
    let id = canvas_claims_unmodified_wheel_id(ctx);
    ctx.data_mut(|data| data.insert_temp(id, claims));
}

pub fn wheel_action(
    delta: Vec2,
    unit: MouseWheelUnit,
    cell_size: Vec2,
    modifiers: Modifiers,
    mode: TermMode,
    point: GridPoint,
) -> Option<WheelAction> {
    let vertical = discrete_scroll_steps(delta.y, unit, cell_size.y);
    let horizontal = discrete_scroll_steps(delta.x, unit, cell_size.x);

    if vertical == 0 && horizontal == 0 {
        return None;
    }

    if !modifiers.shift && mode.intersects(TermMode::MOUSE_MODE) {
        let mut bytes = Vec::new();
        append_wheel_reports(&mut bytes, vertical, horizontal, modifiers, mode, point);
        return Some(WheelAction::Pty(bytes));
    }

    if !modifiers.shift && mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
        let mut bytes = Vec::new();
        append_alternate_scroll(&mut bytes, vertical, horizontal);
        return Some(WheelAction::Pty(bytes));
    }

    if vertical != 0 {
        return Some(WheelAction::Scrollback(vertical));
    }

    None
}

fn append_wheel_reports(
    bytes: &mut Vec<u8>,
    vertical: i32,
    horizontal: i32,
    modifiers: Modifiers,
    mode: TermMode,
    point: GridPoint,
) {
    let vertical_code = if vertical > 0 { 64 } else { 65 };
    for _ in 0..vertical.unsigned_abs() {
        bytes.extend(mouse_report(vertical_code, true, modifiers, mode, point));
    }

    let horizontal_code = if horizontal > 0 { 66 } else { 67 };
    for _ in 0..horizontal.unsigned_abs() {
        bytes.extend(mouse_report(horizontal_code, true, modifiers, mode, point));
    }
}

fn append_alternate_scroll(bytes: &mut Vec<u8>, vertical: i32, horizontal: i32) {
    let vertical_code = if vertical > 0 { b'A' } else { b'B' };
    for _ in 0..vertical.unsigned_abs() {
        bytes.extend_from_slice(b"\x1bO");
        bytes.push(vertical_code);
    }

    let horizontal_code = if horizontal > 0 { b'D' } else { b'C' };
    for _ in 0..horizontal.unsigned_abs() {
        bytes.extend_from_slice(b"\x1bO");
        bytes.push(horizontal_code);
    }
}

fn mouse_report(button: u8, pressed: bool, modifiers: Modifiers, mode: TermMode, point: GridPoint) -> Vec<u8> {
    let mut code = button;
    if modifiers.shift {
        code += 4;
    }
    if modifiers.alt {
        code += 8;
    }
    if control_modifier(modifiers) {
        code += 16;
    }

    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if pressed { 'M' } else { 'm' };
        return format!("\x1b[<{};{};{}{}", code, point.column + 1, point.line + 1, suffix).into_bytes();
    }

    let button = if pressed { code } else { 3 + code };
    let max_point = if mode.contains(TermMode::UTF8_MOUSE) { 2015 } else { 223 };
    let point = if pressed {
        point
    } else {
        GridPoint {
            line: point.line.min(max_point - 1),
            column: point.column.min(max_point - 1),
        }
    };
    normal_mouse_report(point, button, mode.contains(TermMode::UTF8_MOUSE))
}

fn normal_mouse_report(point: GridPoint, button: u8, utf8: bool) -> Vec<u8> {
    let max_point = if utf8 { 2015 } else { 223 };
    if point.line >= max_point || point.column >= max_point {
        return Vec::new();
    }

    let mut bytes = vec![b'\x1b', b'[', b'M', 32 + button];
    append_mouse_position(&mut bytes, point.column, utf8);
    append_mouse_position(&mut bytes, point.line, utf8);
    bytes
}

fn append_mouse_position(bytes: &mut Vec<u8>, position: usize, utf8: bool) {
    if utf8 && position >= 95 {
        let encoded = 32 + 1 + position;
        let first = 0xC0 + encoded / 64;
        let second = 0x80 + (encoded & 63);
        bytes.push(u8::try_from(first).unwrap_or(u8::MAX));
        bytes.push(u8::try_from(second).unwrap_or(u8::MAX));
        return;
    }

    bytes.push(32 + 1 + u8::try_from(position).unwrap_or(u8::MAX));
}

fn discrete_scroll_steps(delta: f32, unit: MouseWheelUnit, cell_extent: f32) -> i32 {
    if !delta.is_finite() {
        return 0;
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    match unit {
        MouseWheelUnit::Line | MouseWheelUnit::Page => delta.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32,
        MouseWheelUnit::Point => {
            if !cell_extent.is_finite() || cell_extent <= 0.0 {
                return 0;
            }
            (delta / cell_extent).round().clamp(i32::MIN as f32, i32::MAX as f32) as i32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        canvas_claims_unmodified_wheel, panel_content_owns_wheel, panel_content_owns_wheel_with_canvas_claim,
        set_canvas_claims_unmodified_wheel,
    };
    use crate::test_egui::DiscardTextures;
    use egui::{Context, Modifiers, RawInput};

    #[test]
    fn unmodified_wheel_belongs_to_the_canvas() {
        assert!(!panel_content_owns_wheel(Modifiers::NONE, false));
        assert!(!panel_content_owns_wheel(Modifiers::CTRL, false));
        assert!(!panel_content_owns_wheel(Modifiers::COMMAND, false));
    }

    #[test]
    fn shift_or_primary_keeps_wheel_on_the_panel() {
        assert!(panel_content_owns_wheel(Modifiers::SHIFT, false));
        assert!(panel_content_owns_wheel(Modifiers::NONE, true));
        assert!(!panel_content_owns_wheel(Modifiers::CTRL | Modifiers::SHIFT, true));
    }

    #[test]
    fn unmodified_wheel_stays_on_the_panel_when_canvas_pan_is_idle() {
        assert!(panel_content_owns_wheel_with_canvas_claim(
            Modifiers::NONE,
            false,
            false
        ));
        assert!(panel_content_owns_wheel_with_canvas_claim(
            Modifiers::SHIFT,
            false,
            false
        ));
        assert!(!panel_content_owns_wheel_with_canvas_claim(
            Modifiers::CTRL,
            false,
            false
        ));
        assert!(!panel_content_owns_wheel_with_canvas_claim(
            Modifiers::COMMAND,
            true,
            false
        ));
    }

    #[test]
    fn canvas_claim_flag_does_not_deadlock_on_context_data() {
        let ctx = Context::default();
        ctx.begin_pass(RawInput::default());
        set_canvas_claims_unmodified_wheel(&ctx, false);
        assert!(!canvas_claims_unmodified_wheel(&ctx));
        set_canvas_claims_unmodified_wheel(&ctx, true);
        assert!(canvas_claims_unmodified_wheel(&ctx));
        let _ = ctx.end_pass().discard_textures();
    }
}

use egui::IconData;

pub const APP_NAME: &str = "Horizon";
pub const APP_ID: &str = "horizon";
pub const APP_TAGLINE: &str = "spatial terminal observatory for multi-agent development";

const ICON_SIZE: u32 = 256;
const BG: [u8; 4] = [10, 22, 34, 255];
const SHELL: [u8; 4] = [17, 32, 48, 255];
const SHELL_STROKE: [u8; 4] = [54, 88, 122, 255];
const PANEL: [u8; 4] = [20, 38, 56, 255];
const PANEL_STROKE: [u8; 4] = [94, 132, 173, 255];
const HORIZON: [u8; 4] = [118, 216, 255, 255];
const SUNSET: [u8; 4] = [255, 170, 114, 255];
const AGENT: [u8; 4] = [126, 225, 198, 255];
const TEXT: [u8; 4] = [239, 245, 251, 255];
const GLOW: [u8; 4] = [70, 122, 196, 138];

#[derive(Clone, Copy)]
struct IconRect {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    radius: f32,
}

pub fn app_icon() -> IconData {
    // Use the embedded 128x128 PNG icon. A 256x256 icon exceeds the X11 base
    // protocol max request size (262,140 bytes) when set via _NET_WM_ICON,
    // causing the icon to silently fail on X11 without BIG-REQUESTS.
    if let Ok(icon) =
        eframe::icon_data::from_png_bytes(include_bytes!(concat!(env!("OUT_DIR"), "/assets/icons/icon-128.png")))
    {
        return icon;
    }
    let mut pixels = vec![0_u8; (ICON_SIZE * ICON_SIZE * 4) as usize];

    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let px = icon_coord(x);
            let py = icon_coord(y);
            let center_x = 128.0;
            let center_y = 116.0;
            let distance = ((px - center_x).powi(2) + (py - center_y).powi(2)).sqrt();
            let vignette = unit_to_u8((distance / 170.0).clamp(0.0, 1.0) * 28.0);
            let base = [
                BG[0].saturating_add(vignette / 4),
                BG[1].saturating_add(vignette / 3),
                BG[2].saturating_add(vignette / 2),
                BG[3],
            ];
            set_pixel(&mut pixels, x, y, base);
        }
    }

    let outer_shell = IconRect {
        left: 24,
        top: 24,
        width: 208,
        height: 208,
        radius: 46.0,
    };
    fill_rounded_rect(&mut pixels, outer_shell, SHELL);
    stroke_rounded_rect(&mut pixels, outer_shell, 3.0, SHELL_STROKE);
    paint_glow(&mut pixels, 128.0, 118.0, 94.0, GLOW);

    stroke_ellipse(&mut pixels, 128.0, 96.0, 56.0, 24.0, 3.0, SHELL_STROKE);
    fill_circle(&mut pixels, 90.0, 74.0, 10.0, AGENT);
    fill_circle(&mut pixels, 90.0, 74.0, 4.0, TEXT);
    fill_circle(&mut pixels, 128.0, 60.0, 10.0, HORIZON);
    fill_circle(&mut pixels, 128.0, 60.0, 4.0, TEXT);
    fill_circle(&mut pixels, 166.0, 74.0, 10.0, SUNSET);
    fill_circle(&mut pixels, 166.0, 74.0, 4.0, TEXT);

    stroke_ellipse(&mut pixels, 128.0, 168.0, 96.0, 42.0, 6.0, HORIZON);
    stroke_ellipse(&mut pixels, 128.0, 180.0, 130.0, 60.0, 4.0, SUNSET);

    let terminal_frame = IconRect {
        left: 66,
        top: 88,
        width: 124,
        height: 84,
        radius: 22.0,
    };
    fill_rounded_rect(&mut pixels, terminal_frame, PANEL);
    stroke_rounded_rect(&mut pixels, terminal_frame, 3.0, PANEL_STROKE);
    draw_line(&mut pixels, 84.0, 108.0, 170.0, 108.0, 4.0, SHELL_STROKE);
    fill_circle(&mut pixels, 82.0, 108.0, 3.0, SUNSET);
    fill_circle(&mut pixels, 94.0, 108.0, 3.0, AGENT);
    fill_circle(&mut pixels, 106.0, 108.0, 3.0, HORIZON);

    let agent_frames = [
        (
            IconRect {
                left: 84,
                top: 122,
                width: 24,
                height: 18,
                radius: 6.0,
            },
            AGENT,
        ),
        (
            IconRect {
                left: 116,
                top: 122,
                width: 24,
                height: 18,
                radius: 6.0,
            },
            HORIZON,
        ),
        (
            IconRect {
                left: 148,
                top: 122,
                width: 24,
                height: 18,
                radius: 6.0,
            },
            SUNSET,
        ),
    ];
    for (frame, stroke) in agent_frames {
        fill_rounded_rect(&mut pixels, frame, SHELL);
        stroke_rounded_rect(&mut pixels, frame, 2.0, stroke);
    }

    draw_line(&mut pixels, 88.0, 152.0, 102.0, 140.0, 5.0, HORIZON);
    draw_line(&mut pixels, 88.0, 152.0, 102.0, 164.0, 5.0, HORIZON);
    draw_line(&mut pixels, 114.0, 152.0, 164.0, 152.0, 6.0, TEXT);

    IconData {
        rgba: pixels,
        width: ICON_SIZE,
        height: ICON_SIZE,
    }
}

#[allow(clippy::cast_precision_loss)]
fn icon_coord(value: u32) -> f32 {
    value as f32
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn unit_to_u8(value: f32) -> u8 {
    value.round().clamp(0.0, f32::from(u8::MAX)) as u8
}

fn set_pixel(buffer: &mut [u8], x: u32, y: u32, color: [u8; 4]) {
    let index = ((y * ICON_SIZE + x) * 4) as usize;
    buffer[index..index + 4].copy_from_slice(&color);
}

fn blend_pixel(buffer: &mut [u8], x: u32, y: u32, color: [u8; 4], alpha: f32) {
    let index = ((y * ICON_SIZE + x) * 4) as usize;
    let amount = alpha.clamp(0.0, 1.0) * (f32::from(color[3]) / 255.0);
    let keep = 1.0 - amount;

    for channel in 0..3 {
        let mixed = (f32::from(buffer[index + channel]) * keep) + (f32::from(color[channel]) * amount);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            buffer[index + channel] = mixed.round().clamp(0.0, 255.0) as u8;
        }
    }
    buffer[index + 3] = 255;
}

fn fill_circle(buffer: &mut [u8], center_x: f32, center_y: f32, radius: f32, color: [u8; 4]) {
    let radius_sq = radius * radius;
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let dx = icon_coord(x) - center_x;
            let dy = icon_coord(y) - center_y;
            if dx * dx + dy * dy <= radius_sq {
                set_pixel(buffer, x, y, color);
            }
        }
    }
}

fn fill_rounded_rect(buffer: &mut [u8], rect: IconRect, color: [u8; 4]) {
    let IconRect {
        left,
        top,
        width,
        height,
        radius,
    } = rect;
    let right = left + width;
    let bottom = top + height;
    let radius_sq = radius * radius;
    let left = icon_coord(left);
    let top = icon_coord(top);
    let right = icon_coord(right);
    let bottom = icon_coord(bottom);

    for y in rect.top..rect.top + rect.height {
        for x in rect.left..rect.left + rect.width {
            let px = icon_coord(x);
            let py = icon_coord(y);
            let inside = if px < left + radius && py < top + radius {
                let dx = px - (left + radius);
                let dy = py - (top + radius);
                dx * dx + dy * dy <= radius_sq
            } else if px > right - radius && py < top + radius {
                let dx = px - (right - radius);
                let dy = py - (top + radius);
                dx * dx + dy * dy <= radius_sq
            } else if px < left + radius && py > bottom - radius {
                let dx = px - (left + radius);
                let dy = py - (bottom - radius);
                dx * dx + dy * dy <= radius_sq
            } else if px > right - radius && py > bottom - radius {
                let dx = px - (right - radius);
                let dy = py - (bottom - radius);
                dx * dx + dy * dy <= radius_sq
            } else {
                true
            };

            if inside {
                set_pixel(buffer, x, y, color);
            }
        }
    }
}

fn stroke_rounded_rect(buffer: &mut [u8], rect: IconRect, thickness: f32, color: [u8; 4]) {
    let IconRect {
        left,
        top,
        width,
        height,
        radius,
    } = rect;
    let right = left + width;
    let bottom = top + height;
    let left = icon_coord(left);
    let top = icon_coord(top);
    let right = icon_coord(right);
    let bottom = icon_coord(bottom);

    for y in rect.top..rect.top + rect.height {
        for x in rect.left..rect.left + rect.width {
            let px = icon_coord(x);
            let py = icon_coord(y);
            let outer = rounded_rect_distance(px, py, left, top, right, bottom, radius);
            let inner = rounded_rect_distance(
                px,
                py,
                left + thickness,
                top + thickness,
                right - thickness,
                bottom - thickness,
                (radius - thickness).max(0.0),
            );

            if outer <= 0.0 && inner > 0.0 {
                set_pixel(buffer, x, y, color);
            }
        }
    }
}

fn rounded_rect_distance(x: f32, y: f32, left: f32, top: f32, right: f32, bottom: f32, radius: f32) -> f32 {
    let qx = (x - x.clamp(left + radius, right - radius)).abs();
    let qy = (y - y.clamp(top + radius, bottom - radius)).abs();
    (qx * qx + qy * qy).sqrt() - radius
}

fn stroke_ellipse(
    buffer: &mut [u8],
    center_x: f32,
    center_y: f32,
    radius_x: f32,
    radius_y: f32,
    thickness: f32,
    color: [u8; 4],
) {
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let dx = (icon_coord(x) - center_x) / radius_x;
            let dy = (icon_coord(y) - center_y) / radius_y;
            let value = (dx * dx + dy * dy).sqrt();
            if (value - 1.0).abs() <= thickness / radius_x.max(radius_y) {
                set_pixel(buffer, x, y, color);
            }
        }
    }
}

fn draw_line(buffer: &mut [u8], from_x: f32, from_y: f32, to_x: f32, to_y: f32, thickness: f32, color: [u8; 4]) {
    let line_length_sq = (to_x - from_x).powi(2) + (to_y - from_y).powi(2);
    let radius = thickness * 0.5;

    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let px = icon_coord(x);
            let py = icon_coord(y);
            let projection = (((px - from_x) * (to_x - from_x)) + ((py - from_y) * (to_y - from_y))) / line_length_sq;
            let t = projection.clamp(0.0, 1.0);
            let nearest_x = from_x + (to_x - from_x) * t;
            let nearest_y = from_y + (to_y - from_y) * t;
            let distance = ((px - nearest_x).powi(2) + (py - nearest_y).powi(2)).sqrt();

            if distance <= radius {
                set_pixel(buffer, x, y, color);
            }
        }
    }
}

fn paint_glow(buffer: &mut [u8], center_x: f32, center_y: f32, radius: f32, color: [u8; 4]) {
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let distance = ((icon_coord(x) - center_x).powi(2) + (icon_coord(y) - center_y).powi(2)).sqrt();
            if distance <= radius {
                let alpha = 1.0 - (distance / radius);
                blend_pixel(buffer, x, y, color, alpha * 0.45);
            }
        }
    }
}

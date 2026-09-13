//! RGB8 page frames to 4:2:0 YUV at a locked encode size.

const MIN_ALIGN: u32 = 16;

#[derive(Debug)]
pub(super) struct YuvFrame {
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

#[must_use]
pub(super) fn encode_size(src_width: u32, src_height: u32, max_width: u32) -> (u32, u32) {
    let max_width = max_width.max(MIN_ALIGN);
    let longest = src_width.max(src_height).max(1);
    let (width, height) = if longest > max_width {
        (
            src_width.saturating_mul(max_width) / longest,
            src_height.saturating_mul(max_width) / longest,
        )
    } else {
        (src_width, src_height)
    };
    (align_down(width), align_down(height))
}

fn align_down(value: u32) -> u32 {
    value.max(MIN_ALIGN) & !7
}

#[must_use]
pub(super) fn rgb_to_yuv420(rgb: &[u8], src_width: u32, src_height: u32, dst_width: u32, dst_height: u32) -> YuvFrame {
    let dst_width = align_down(dst_width);
    let dst_height = align_down(dst_height);
    let y_len = usize::try_from(dst_width.saturating_mul(dst_height)).unwrap_or(0);
    let uv_len = usize::try_from((dst_width / 2).saturating_mul(dst_height / 2)).unwrap_or(0);
    let mut y = vec![0_u8; y_len];
    let mut u = vec![128_u8; uv_len];
    let mut v = vec![128_u8; uv_len];
    let src_pixels = usize::try_from(src_width.saturating_mul(src_height)).unwrap_or(0);
    if src_width == 0 || src_height == 0 || rgb.len() < src_pixels.saturating_mul(3) {
        return YuvFrame { y, u, v };
    }

    let (content_w, content_h) = fitted_content(src_width, src_height, dst_width, dst_height);
    let x_off = dst_width.saturating_sub(content_w) / 2;
    let y_off = dst_height.saturating_sub(content_h) / 2;
    let dst_w = usize::try_from(dst_width).unwrap_or(0);
    let src_w = usize::try_from(src_width).unwrap_or(1);

    for dy in 0..dst_height {
        for dx in 0..dst_width {
            if dx < x_off || dy < y_off || dx >= x_off + content_w || dy >= y_off + content_h {
                continue;
            }
            let sx = ((dx - x_off) * src_width / content_w.max(1)).min(src_width.saturating_sub(1));
            let sy = ((dy - y_off) * src_height / content_h.max(1)).min(src_height.saturating_sub(1));
            let offset = (usize::try_from(sy).unwrap_or(0) * src_w + usize::try_from(sx).unwrap_or(0)) * 3;
            let Some([red, green, blue]) = rgb.get(offset..offset + 3).and_then(|pixel| pixel.first_chunk::<3>())
            else {
                continue;
            };
            let (luma, cb, cr) = rgb_to_ycbcr(*red, *green, *blue);
            let y_index = usize::try_from(dy).unwrap_or(0) * dst_w + usize::try_from(dx).unwrap_or(0);
            if let Some(slot) = y.get_mut(y_index) {
                *slot = luma;
            }
            if dy % 2 == 0 && dx % 2 == 0 {
                let uv_index =
                    usize::try_from(dy / 2).unwrap_or(0) * (dst_w / 2) + usize::try_from(dx / 2).unwrap_or(0);
                if let Some(slot) = u.get_mut(uv_index) {
                    *slot = cb;
                }
                if let Some(slot) = v.get_mut(uv_index) {
                    *slot = cr;
                }
            }
        }
    }

    YuvFrame { y, u, v }
}

fn fitted_content(src_width: u32, src_height: u32, dst_width: u32, dst_height: u32) -> (u32, u32) {
    let src_width = src_width.max(1);
    let src_height = src_height.max(1);
    if dst_width.saturating_mul(src_height) <= dst_height.saturating_mul(src_width) {
        (dst_width, src_height.saturating_mul(dst_width) / src_width)
    } else {
        (src_width.saturating_mul(dst_height) / src_height, dst_height)
    }
}

fn rgb_to_ycbcr(red: u8, green: u8, blue: u8) -> (u8, u8, u8) {
    let red = i32::from(red);
    let green = i32::from(green);
    let blue = i32::from(blue);
    let y = (77 * red + 150 * green + 29 * blue) >> 8;
    let cb = 128 + ((-43 * red - 85 * green + 128 * blue) >> 8);
    let cr = 128 + ((128 * red - 107 * green - 21 * blue) >> 8);
    (clamp_u8(y), clamp_u8(cb), clamp_u8(cr))
}

fn clamp_u8(value: i32) -> u8 {
    u8::try_from(value.clamp(0, 255)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_size_aligns_and_respects_max_width() {
        assert_eq!(encode_size(1280, 800, 1280), (1280, 800));
        let (width, height) = encode_size(1920, 1080, 1280);
        assert!(width <= 1280);
        assert_eq!(width % 8, 0);
        assert_eq!(height % 8, 0);
        assert!(width >= MIN_ALIGN);
        assert!(height >= MIN_ALIGN);
    }

    #[test]
    fn solid_red_frame_has_expected_luma_and_chroma() {
        let rgb = [255_u8, 0, 0].repeat(32 * 32);
        let frame = rgb_to_yuv420(&rgb, 32, 32, 32, 32);
        let (luma, cb, cr) = rgb_to_ycbcr(255, 0, 0);
        assert!(frame.y.iter().all(|value| *value == luma));
        assert!(frame.u.iter().all(|value| *value == cb));
        assert!(frame.v.iter().all(|value| *value == cr));
    }
}

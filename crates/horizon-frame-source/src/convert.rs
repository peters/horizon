//! NV12 pictures as the RGBA pixels of a frame, in exact integer arithmetic.
//!
//! The colours of the contract are BT.709 with limited range: the black of the
//! luma plane is 16, the white is 235, and both chroma planes are 16 to 240 around
//! a level of 128. The conversion of one pixel is therefore
//!
//! ```text
//! R = 1.164384 (Y - 16) + 1.792741 (Cr - 128)
//! G = 1.164384 (Y - 16) - 0.213249 (Cb - 128) - 0.532909 (Cr - 128)
//! B = 1.164384 (Y - 16) + 2.112402 (Cb - 128)
//! ```
//!
//! with each coefficient held as a whole number of 1/65536, so the value of one
//! pixel is exactly the value of that formula and no floating-point rounding
//! differs between two computers. Chroma is shared by each 2x2 block of luma, as
//! the interleaved UV plane holds one pair for each block.
use horizon_vnc::Frame;

use crate::{Error, picture::Picture};

/// 1.164384, the luma coefficient of BT.709 limited range, in 1/65536.
const LUMA: i32 = 76_304;
/// 1.792741, the Cr coefficient of red.
const CR_TO_RED: i32 = 117_478;
/// 0.213249, the Cb coefficient of green.
const CB_TO_GREEN: i32 = 13_975;
/// 0.532909, the Cr coefficient of green.
const CR_TO_GREEN: i32 = 34_925;
/// 2.112402, the Cb coefficient of blue.
const CB_TO_BLUE: i32 = 138_438;
/// One half, so a value rounds to the nearer whole number and not towards zero.
const HALF: i32 = 32_768;

/// The level of black in the luma plane of limited range.
pub const BLACK: u8 = 16;

/// The level of white in the luma plane of limited range.
pub const WHITE: u8 = 235;

/// One picture as a frame of RGBA pixels.
///
/// # Errors
/// Reports a plane that is not the size that its header names.
pub fn frame(picture: &Picture) -> Result<Frame, Error> {
    let [width, height] = picture.header.size();
    let mut pixels = vec![0_u32; width * height];
    convert(picture.y(), picture.uv(), width, &mut pixels)?;
    Ok(Frame::new([width, height], pixels)?)
}

/// Convert one NV12 picture into `pixels`, which holds width times height pixels.
///
/// Each term of the formula is precomputed for its one byte, so a pixel costs three
/// lookups, two additions and the clamp of a channel. The tables are built from the
/// constants on each call: 768 whole numbers against a picture of millions of
/// pixels, and no state that two threads share.
///
/// # Errors
/// Reports planes that are not the size of the picture.
pub fn convert(y: &[u8], uv: &[u8], width: usize, pixels: &mut [u32]) -> Result<(), Error> {
    let height = pixels.len().checked_div(width).unwrap_or(0);
    if width == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) || height == 0 {
        return Err(Error::Frame("the size of a picture is not one even picture".into()));
    }
    if y.len() != width * height || uv.len() != width * height / 2 {
        return Err(Error::Frame("the planes of a picture are not the size of its header".into()));
    }
    let tables = Tables::new();
    for row in 0..height {
        let luma_row = &y[row * width..(row + 1) * width];
        let chroma_row = &uv[(row / 2) * width..(row / 2 + 1) * width];
        let pixels_row = &mut pixels[row * width..(row + 1) * width];
        for (column, pixel) in pixels_row.iter_mut().enumerate() {
            let luma = tables.luma[usize::from(luma_row[column])];
            let cb = usize::from(chroma_row[column & !1]);
            let cr = usize::from(chroma_row[column | 1]);
            let red = channel(luma + tables.cr_red[cr]);
            let green = channel(luma + tables.cb_green[cb] + tables.cr_green[cr]);
            let blue = channel(luma + tables.cb_blue[cb]);
            *pixel = u32::from_ne_bytes([red, green, blue, 255]);
        }
    }
    Ok(())
}

/// Each term of the formula for its one byte, in the 1/65536 of the coefficients.
struct Tables {
    luma: [i32; 256],
    cr_red: [i32; 256],
    cr_green: [i32; 256],
    cb_green: [i32; 256],
    cb_blue: [i32; 256],
}

impl Tables {
    fn new() -> Self {
        let mut tables = Self {
            luma: [0; 256],
            cr_red: [0; 256],
            cr_green: [0; 256],
            cb_green: [0; 256],
            cb_blue: [0; 256],
        };
        for value in 0..256_usize {
            let level = i32::try_from(value).unwrap_or(0);
            let chroma = level - 128;
            tables.luma[value] = LUMA * (level - i32::from(BLACK));
            tables.cr_red[value] = CR_TO_RED * chroma;
            tables.cr_green[value] = -CR_TO_GREEN * chroma;
            tables.cb_green[value] = -CB_TO_GREEN * chroma;
            tables.cb_blue[value] = CB_TO_BLUE * chroma;
        }
        tables
    }
}

/// One channel of a pixel, rounded and clamped to one byte.
fn channel(value: i32) -> u8 {
    let value = ((value + HALF) >> 16).clamp(0, 255);
    u8::try_from(value).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picture::{Header, Picture};

    fn pixel_of(y: u8, cb: u8, cr: u8) -> [u8; 4] {
        let mut pixels = [0_u32; 4];
        convert(&[y; 4], &[cb, cr], 2, &mut pixels).expect("one picture of 2x2");
        assert!(pixels.iter().all(|pixel| *pixel == pixels[0]), "one chroma for the whole block");
        pixels[0].to_ne_bytes()
    }

    #[test]
    fn white_black_and_the_colours_of_the_contract_are_their_own_values() {
        assert_eq!(pixel_of(WHITE, 128, 128), [255, 255, 255, 255], "white");
        assert_eq!(pixel_of(BLACK, 128, 128), [0, 0, 0, 255], "black");
        // The 8-bit limited values of red, green and blue are the nearest whole
        // numbers of the formula: (63, 102, 240), (173, 42, 26) and (32, 240, 118).
        assert_eq!(pixel_of(63, 102, 240), [255, 1, 0, 255], "red");
        assert_eq!(pixel_of(173, 42, 26), [0, 255, 1, 255], "green");
        assert_eq!(pixel_of(32, 240, 118), [1, 0, 255, 255], "blue");
        // A level below black and above white is clamped and never wraps.
        assert_eq!(pixel_of(0, 128, 128), [0, 0, 0, 255]);
        assert_eq!(pixel_of(255, 128, 128), [255, 255, 255, 255]);
        assert_eq!(pixel_of(128, 0, 0), [0, 226, 0, 255], "both chroma at their smallest");
    }

    #[test]
    fn a_gradient_of_luma_is_one_byte_per_step_and_never_a_wrap() {
        let values: Vec<u8> = (BLACK..=WHITE)
            .flat_map(|y| pixel_of(y, 128, 128)[..3].to_vec())
            .collect();
        for window in values.as_chunks::<3>().0.windows(2) {
            assert!(window[0][0] <= window[1][0], "the gradient rises: {window:?}");
        }
        assert_eq!(values[..3], [0, 0, 0], "black");
        assert_eq!(values[values.len() - 3..], [255, 255, 255], "white");
        // The mid grey of limited range is a little above the same number.
        let mid = pixel_of(126, 128, 128);
        assert_eq!((mid[0], mid[1], mid[2]), (128, 128, 128), "{mid:?}");
    }

    #[test]
    fn each_pixel_of_a_picture_takes_the_chroma_of_its_own_block() {
        // A 4x2 picture: the left block is black with a red chroma, the right
        // block is white with a blue chroma.
        let y = [16, 16, 235, 235, 16, 16, 235, 235];
        let uv = [102, 240, 240, 118];
        let mut pixels = vec![0_u32; 8];
        convert(&y, &uv, 4, &mut pixels).expect("one picture of 4x2");
        let colors: Vec<[u8; 4]> = pixels.iter().map(|pixel| pixel.to_ne_bytes()).collect();
        assert_eq!(colors[0], colors[1]);
        assert_eq!(colors[0], colors[4], "the second row shares the same blocks");
        assert_eq!(colors[2], colors[3]);
        assert_eq!(colors[2], colors[6]);
        assert_ne!(colors[0], colors[2], "the two blocks have different chroma");
    }

    #[test]
    fn a_picture_becomes_one_frame_of_its_own_size() {
        let header = Header {
            sequence: 1,
            captured_at_micros: 2,
            width: 16,
            height: 16,
        };
        let picture = Picture {
            header,
            planes: vec![128; 16 * 16 + 16 * 16 / 2],
        };
        let image = frame(&picture).expect("one frame");
        assert_eq!(image.size(), [16, 16]);
        assert_eq!(image.pixels().len(), 16 * 16);
        assert_eq!(image.pixels()[0].to_ne_bytes(), [130, 130, 130, 255]);

        let short = Picture {
            header,
            planes: vec![0; 4],
        };
        frame(&short).expect_err("planes that are not the size of the header");
    }

    // A measure of the conversion of one 1920x1080 picture on the machine that
    // runs the tests. The bound of the design is 8 ms; this test reports the
    // time and asserts nothing about it, so a slow machine is not a failure.
    #[test]
    fn the_conversion_of_one_1080p_picture_is_measured() {
        let (width, height) = (1920, 1080);
        let y = vec![126_u8; width * height];
        let uv = vec![128_u8; width * height / 2];
        let mut pixels = vec![0_u32; width * height];
        // One warm round, so the measure is the conversion and not the first
        // touch of the memory of one picture, and then the fastest of five rounds,
        // which is the round that another program of the machine disturbed least.
        convert(&y, &uv, width, &mut pixels).expect("one picture");
        let mut fastest = std::time::Duration::MAX;
        for _ in 0..5 {
            let started = std::time::Instant::now();
            convert(&y, &uv, width, &mut pixels).expect("one picture");
            fastest = fastest.min(started.elapsed());
        }
        println!(
            "NV12 to RGBA of one 1920x1080 picture: {:.2} ms",
            fastest.as_secs_f64() * 1000.0
        );
    }
}
